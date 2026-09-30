//! # Escrow Contract
//!
//! Trustless chess-match escrow on Stellar Soroban. Players stake XLM or USDC before a game;
//! the verified winner is paid out automatically the moment the oracle submits the result.
//!
//! ## State Machine
//!
//! Every match moves through the following states. Invalid transitions are rejected on-chain
//! with [`Error::InvalidState`].
//!
//! ```text
//! (none) ──create_match()──► Pending
//!                                │
//!                ┌───────────────┤
//!                │               │
//!         cancel_match()    deposit() × 2
//!                │               │
//!                ▼               ▼
//!            Cancelled        Active ──── claim_timeout() ──► Cancelled
//!                           (funds held)         │
//!                                │        cancel_match()
//!                                │        (both players) ──► Cancelled
//!                         submit_result()
//!                          (oracle only)
//!                                │
//!                                ▼
//!                         PendingResult  ◄── override_result() (admin)
//!                          (dispute window)
//!                                │
//!                       finalize_result()
//!                     (after window expires)
//!                                │
//!                                ▼
//!                           Completed
//!                          (payout done)
//! ```
//!
//! `Cancelled` and `Completed` are **terminal** — no further transitions are possible.
//!
//! ## Key Data Structures
//!
//! - [`Match`](types::Match) — full record of a single betting match stored in persistent storage.
//! - [`MatchState`](types::MatchState) — lifecycle enum driving the state machine above.
//! - [`Winner`](types::Winner) — payout outcome: `Player1`, `Player2`, or `Draw`.
//! - [`Platform`](types::Platform) — chess platform the game is hosted on (`Lichess` / `ChessDotCom`).
//! - [`DataKey`](types::DataKey) — all storage keys used by the contract.
//! - [`Error`](errors::Error) — every error code the contract can return.
//!
//! ## Further Reading
//!
//! - Architecture & sequence diagrams: [`docs/architecture.md`](../../docs/architecture.md)
//! - Full API reference with CLI examples: [`docs/api-reference.md`](../../docs/api-reference.md)
//! - Emergency procedures: [`docs/runbook.md`](../../docs/runbook.md)

// Enforced for every item in this crate, and denied outright by the `Rustdoc`
// CI job. The two `#[allow(missing_docs)]` attributes below are the only
// exemptions: they cover items that soroban's `#[contract]` and `#[contractimpl]`
// macros generate, which cannot be documented from this crate's source.
#![warn(missing_docs)]
#![no_std]

mod constants;
mod errors;
mod types;

pub use constants::*;

use errors::Error;
use soroban_sdk::{contract, contractimpl, symbol_short, token, vec, Address, Env, String, Symbol, TryFromVal, Vec};
use types::{DataKey, Match, MatchState, OptionalWinner, Platform, Winner};

fn is_zero_address(env: &Env, addr: &Address) -> bool {
    // The all-zeros Stellar account key encodes to this strkey.
    // We construct the Address via the ScAddress XDR path since
    // TryFromVal<Env, String> is not implemented for Address.
    use soroban_sdk::xdr::{AccountId, PublicKey, ScAddress, Uint256};
    let zero_key = Uint256([0u8; 32]);
    let sc_addr = ScAddress::Account(AccountId(PublicKey::PublicKeyTypeEd25519(zero_key)));
    let zero_address = Address::try_from_val(env, &sc_addr).expect("invalid zero address");
    addr == &zero_address
}

/// The trustless chess-match escrow contract.
///
/// This unit struct is a marker and carries no state of its own. All state lives
/// in the contract's storage, addressed by the `DataKey` enum in `types.rs`, and
/// every operation is exposed as an associated function in the `#[contractimpl]`
/// block below.
///
/// The public data model is:
///
/// | Concept            | Type                                                    |
/// |--------------------|---------------------------------------------------------|
/// | Per-match state    | `Match` (persistent, keyed by `DataKey::Match`)          |
/// | Lifecycle          | `MatchState`                                            |
/// | Oracle result      | `Winner`, wrapped in `OptionalWinner` while in dispute  |
/// | Storage keys       | `DataKey`                                               |
/// | Error codes        | `Error` in `errors.rs`                                  |
///
/// See the crate-level documentation above for the full state machine.
#[allow(missing_docs)]
#[contract]
pub struct EscrowContract;

#[allow(missing_docs)]
#[contractimpl]
impl EscrowContract {
    /// Return whether the contract is currently paused.
    pub fn is_paused(env: &Env) -> bool {
        env.storage()
            .instance()
            .get(&DataKey::Paused)
            .unwrap_or(false)
    }

    /// Return the configured dispute window duration in ledgers.
    ///
    /// This is the time the admin has to call [`override_result`](EscrowContract::override_result)
    /// after the oracle submits a result, before it becomes final. Defaults to
    /// `DISPUTE_WINDOW_LEDGERS` (~24 hours) if not configured at initialization.
    pub fn get_dispute_window_ledgers(env: &Env) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::DisputeWindowLedgers)
            .unwrap_or(DISPUTE_WINDOW_LEDGERS)
    }

    /// Return the configured match timeout duration in ledgers.
    ///
    /// This is the time an active match can remain without an oracle result
    /// before either player may call [`claim_timeout`](EscrowContract::claim_timeout).
    /// Defaults to `TIMEOUT_LEDGERS` (~7 days) if not configured at initialization.
    pub fn get_timeout_ledgers(env: &Env) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::TimeoutLedgers)
            .unwrap_or(TIMEOUT_LEDGERS)
    }

    fn get_match_count(env: &Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::MatchCount)
            .unwrap_or(0)
    }

    /// Return the total number of matches ever created.
    ///
    /// Exposed as a public read-only view so that frontends and off-chain tooling
    /// can efficiently query the total match count for pagination and progress
    /// displays without needing to enumerate via `list_matches`.
    pub fn match_count(env: Env) -> u64 {
        Self::get_match_count(&env)
    }

    fn validate_match_id(env: &Env, match_id: u64) -> Result<(), Error> {
        if match_id >= Self::get_match_count(env) {
            return Err(Error::MatchNotFound);
        }
        Ok(())
    }

    /// Validate that every byte of `game_id` belongs to the set `[A-Za-z0-9_-]`.
    ///
    /// This enforces printable ASCII and prevents null bytes, control characters,
    /// whitespace, and non-ASCII sequences from entering persistent storage.
    /// The allowed set mirrors the identifier format used by Lichess and Chess.com.
    ///
    /// The string is copied into a fixed 64-byte stack buffer — the same size as
    /// `MAX_GAME_ID_LEN` — so no heap allocation is required inside the WASM guest.
    fn is_valid_game_id(game_id: &soroban_sdk::String) -> bool {
        let len = game_id.len() as usize;
        // Safety: len is already validated to be in [1, MAX_GAME_ID_LEN].
        let mut buf = [0u8; MAX_GAME_ID_LEN as usize];
        game_id.copy_into_slice(&mut buf[..len]);
        for i in 0..len {
            let b = buf[i];
            let ok = b.is_ascii_alphanumeric() || b == b'_' || b == b'-';
            if !ok {
                return false;
            }
        }
        true
    }

    /// Extend the lifetime of every instance-storage entry held by this contract.
    ///
    /// Instance storage (oracle, admin, token, paused, match_count) shares a single
    /// TTL, so it is bumped as a unit after every mutating call. Without this the
    /// instance entries would expire while a long-running tournament of matches was
    /// still in progress and the contract would start failing with storage-not-found
    /// errors.
    fn bump_instance_ttl(env: &Env) {
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Pre-flight check that the contract retains at least
    /// [`ESCROW_RESERVE_BUFFER_STROOPS`] of `token` **after** a total payout of
    /// `payout_total` is subtracted.
    ///
    /// Without this guard a token::transfer performed at payout time could
    /// fail at the Stellar protocol layer with a reserve-induced
    /// `op_under_min_balance` / generic `TransferFailed`, leaving the match
    /// state machine stuck (funds not sent, state still `PendingResult` etc.).
    fn ensure_reserve_for_payout(
        env: &Env,
        token: &Address,
        payout_total: i128,
    ) -> Result<(), Error> {
        let client = token::Client::new(env, token);
        let balance = client.balance(&env.current_contract_address());
        if balance < payout_total + ESCROW_RESERVE_BUFFER_STROOPS {
            return Err(Error::InsufficientReserve);
        }
        Ok(())
    }

    /// Initialize the contract with a trusted oracle address, an admin, a default token,
    /// and a pre-registered safe address that is the sole permitted destination for
    /// [`emergency_drain`](EscrowContract::emergency_drain).
    ///
    /// The `safe_address` is stored immutably at initialization time. It cannot be
    /// changed afterwards. This eliminates the rug-pull vector where a compromised or
    /// malicious admin could call `emergency_drain` with an arbitrary destination address.
    ///
    /// The `dispute_window_ledgers` and `timeout_ledgers` parameters allow configurable
    /// timing for different deployments (testnet vs mainnet, casual vs high-stakes matches).
    /// If not provided, they default to `DISPUTE_WINDOW_LEDGERS` (~24 hours) and
    /// `TIMEOUT_LEDGERS` (~7 days) respectively.
    ///
    /// # Arguments
    ///
    /// * `oracle` — The address of the trusted oracle contract.
    /// * `admin` — The address of the contract administrator.
    /// * `token` — The SEP-41 token address for staking.
    /// * `safe_address` — The immutable destination for emergency drain operations.
    /// * `dispute_window_ledgers` — The duration of the dispute window in ledgers. If `None`,
    ///   defaults to `DISPUTE_WINDOW_LEDGERS`.
    /// * `timeout_ledgers` — The timeout duration for active matches in ledgers. If `None`,
    ///   defaults to `TIMEOUT_LEDGERS`.
    ///
    /// # Panics
    ///
    /// Panics with `"Contract already initialized"` if called more than once.
    pub fn initialize(
        env: Env,
        oracle: Address,
        admin: Address,
        token: Address,
        safe_address: Address,
        dispute_window_ledgers: Option<u32>,
        timeout_ledgers: Option<u32>,
    ) -> Result<(), Error> {
        if env.storage().instance().has(&DataKey::Oracle) {
            return Err(Error::AlreadyInitialized);
        }
        let token_client = token::Client::new(&env, &token);
        let _ = token_client.decimals();
        env.storage().instance().set(&DataKey::Oracle, &oracle);
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Token, &token);

        // Seed the allowlist with the default token so a contract that is
        // never extended behaves exactly as it did before the allowlist
        // existed: the only accepted token is the one passed to `initialize`.
        // `remove_token` refuses to remove this entry, so the contract can
        // never be left with no acceptable token at all.
        env.storage()
            .persistent()
            .set(&DataKey::TokenAllowlisted(token.clone()), &true);
        env.storage().persistent().extend_ttl(
            &DataKey::TokenAllowlisted(token.clone()),
            MATCH_TTL_LEDGERS,
            MATCH_TTL_LEDGERS,
        );
        env.storage()
            .instance()
            .set(&DataKey::SafeAddress, &safe_address);
        env.storage().instance().set(&DataKey::MatchCount, &0u64);
        env.storage().instance().set(&DataKey::Paused, &false);

        // Store the configured dispute window, or use the default
        let dispute_window = dispute_window_ledgers.unwrap_or(DISPUTE_WINDOW_LEDGERS);
        env.storage()
            .instance()
            .set(&DataKey::DisputeWindowLedgers, &dispute_window);

        // Store the configured timeout, or use the default
        let timeout = timeout_ledgers.unwrap_or(TIMEOUT_LEDGERS);
        env.storage()
            .instance()
            .set(&DataKey::TimeoutLedgers, &timeout);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
        Ok(())
    }

    /// Rotate the oracle address — requires the current admin to authorize.
    pub fn update_oracle(env: Env, new_oracle: Address) -> Result<(), Error> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::Unauthorized)?;
        admin.require_auth();
        let old_oracle: Address = env
            .storage()
            .instance()
            .get(&DataKey::Oracle)
            .ok_or(Error::Unauthorized)?;
        env.storage().instance().set(&DataKey::Oracle, &new_oracle);
        Self::bump_instance_ttl(&env);
        env.events().publish(
            (
                Symbol::new(&env, "admin"),
                Symbol::new(&env, "oracle_updated"),
            ),
            (old_oracle, new_oracle, admin),
        );
        Ok(())
    }

    /// Rotate the admin address — requires the current admin to authorize.
    pub fn transfer_admin(env: Env, caller: Address, new_admin: Address) -> Result<(), Error> {
        let current_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::Unauthorized)?;

        if caller != current_admin {
            return Err(Error::Unauthorized);
        }
        caller.require_auth();

        if is_zero_address(&env, &new_admin) || new_admin == current_admin {
            return Err(Error::InvalidAdmin);
        }

        env.storage().instance().set(&DataKey::Admin, &new_admin);
        Self::bump_instance_ttl(&env);
        env.events().publish(
            (Symbol::new(&env, "admin"), symbol_short!("adm_xfer")),
            (current_admin, new_admin),
        );
        Ok(())
    }

    /// Pause the contract — admin only. Blocks create_match, deposit, and submit_result.
    pub fn pause(env: Env) -> Result<(), Error> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::Unauthorized)?;
        admin.require_auth();
        if Self::is_paused(env.clone()) {
            return Ok(());
        }
        env.storage().instance().set(&DataKey::Paused, &true);
        Self::bump_instance_ttl(&env);
        env.events().publish(
            (Symbol::new(&env, "admin"), symbol_short!("paused")),
            (admin, env.ledger().sequence()),
        );
        Ok(())
    }

    /// Unpause the contract — admin only.
    pub fn unpause(env: Env) -> Result<(), Error> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::Unauthorized)?;
        admin.require_auth();
        if !Self::is_paused(env.clone()) {
            return Ok(());
        }
        env.storage().instance().set(&DataKey::Paused, &false);
        Self::bump_instance_ttl(&env);
        env.events().publish(
            (Symbol::new(&env, "admin"), symbol_short!("unpaused")),
            (admin, env.ledger().sequence()),
        );
        Ok(())
    }

    /// Create a new match. Both players must call `deposit` before the game starts.
    ///
    /// `token` selects which SEP-41 token this match is escrowed in. Passing
    /// `None` uses the contract's default token (the one supplied to
    /// [`initialize`](EscrowContract::initialize)), which is what a caller that
    /// does not care about the currency should do.
    ///
    /// A non-`None` token must be on the admin-managed allowlist
    /// ([`add_token`](EscrowContract::add_token)); otherwise the call fails with
    /// [`Error::TokenNotAllowlisted`]. This is what stops a caller from naming
    /// an arbitrary SEP-41 contract and having the escrow pull funds through
    /// it.
    ///
    /// The chosen token is copied into [`Match::token`] at creation and is the
    /// token [`deposit`](EscrowContract::deposit) pulls and the payout pays
    /// out in. A later `remove_token` does **not** disturb matches that already
    /// exist: it stops new matches from being created in that token, leaving
    /// in-flight escrows untouched and settleable.
    pub fn create_match(
        env: Env,
        player1: Address,
        player2: Address,
        stake_amount: i128,
        token: Option<Address>,
        game_id: String,
        platform: Platform,
    ) -> Result<u64, Error> {
        player1.require_auth();

        if Self::is_paused(&env) {
            return Err(Error::ContractPaused);
        }
        // Reject a zero stake explicitly before the MIN_STAKE range check so
        // callers receive a dedicated error code rather than StakeTooLow.
        if stake_amount == 0 {
            return Err(Error::InvalidStakeAmount);
        }
        if stake_amount < MIN_STAKE {
            return Err(Error::StakeTooLow);
        }
        if stake_amount > MAX_STAKE {
            return Err(Error::StakeTooHigh);
        }
        if player1 == player2 {
            return Err(Error::InvalidPlayers);
        }
        // Validate that neither player is the zero/burn address.
        // Uses the existing is_zero_address helper (XDR-based Address equality)
        // instead of a string comparison, which is cheaper on compute budget and
        // robust against any future strkey encoding changes.
        if is_zero_address(&env, &player1) || is_zero_address(&env, &player2) {
            return Err(Error::InvalidAddress);
        }
        let game_id_len = game_id.len();
        if game_id_len == 0 || game_id_len > MAX_GAME_ID_LEN {
            return Err(Error::InvalidGameId);
        }
        // Enforce printable ASCII: only [A-Za-z0-9_-] are accepted.
        // This closes the gap where a caller could submit a game_id containing
        // null bytes, control characters, or non-ASCII sequences that would
        // match the length check but silently diverge from the platform API
        // lookup key used by the oracle.
        if !Self::is_valid_game_id(&game_id) {
            return Err(Error::InvalidGameId);
        }
        if env
            .storage()
            .persistent()
            .has(&DataKey::GameId(game_id.clone()))
        {
            return Err(Error::DuplicateGameId);
        }

        // Resolve the token: an explicit argument overrides the default, and
        // `None` means "use whatever this contract was initialized with".
        let stored_token: Address = env
            .storage()
            .instance()
            .get(&DataKey::Token)
            .ok_or(Error::Unauthorized)?;
        let token = match token {
            Some(t) => t,
            None => stored_token,
        };

        // The default token is allowlisted by `initialize` and cannot be
        // removed, so checking every token — including the default — against
        // the allowlist is both simpler and stricter than special-casing it.
        if !env
            .storage()
            .persistent()
            .has(&DataKey::TokenAllowlisted(token.clone()))
        {
            return Err(Error::TokenNotAllowlisted);
        }

        let id: u64 = env
            .storage()
            .instance()
            .get(&DataKey::MatchCount)
            .unwrap_or(0);

        if env.storage().persistent().has(&DataKey::Match(id)) {
            return Err(Error::AlreadyExists);
        }

        // Capture the event values *before* the locals are moved into the Match struct.
        let event_player1 = player1.clone();
        let event_player2 = player2.clone();
        let event_game_id = game_id.clone();

        let m = Match {
            id,
            player1,
            player2,
            stake_amount,
            token,
            game_id,
            platform,
            state: MatchState::Pending,
            player1_deposited: false,
            player2_deposited: false,
            created_ledger: env.ledger().sequence(),
            activated_ledger: None,
            pending_result_ledger: None,
            pending_winner: OptionalWinner::None,
            cancelled_ledger: None,
            completed_ledger: None,
        };

        env.storage().persistent().set(&DataKey::Match(id), &m);
        env.storage().persistent().extend_ttl(
            &DataKey::Match(id),
            MATCH_TTL_LEDGERS,
            MATCH_TTL_LEDGERS,
        );
        env.storage()
            .persistent()
            .set(&DataKey::GameId(m.game_id.clone()), &id);
        env.storage().persistent().extend_ttl(
            &DataKey::GameId(m.game_id.clone()),
            MATCH_TTL_LEDGERS,
            MATCH_TTL_LEDGERS,
        );
        let next_id = id.checked_add(1).ok_or(Error::Overflow)?;
        env.storage().instance().set(&DataKey::MatchCount, &next_id);
        Self::bump_instance_ttl(&env);

        env.events().publish(
            (Symbol::new(&env, "match"), symbol_short!("created")),
            (id, event_player1, event_player2, stake_amount, event_game_id),
        );

        Ok(id)
    }

    /// Player deposits their stake into escrow.
    pub fn deposit(env: Env, match_id: u64, player: Address) -> Result<(), Error> {
        player.require_auth();

        if Self::is_paused(&env) {
            return Err(Error::ContractPaused);
        }

        let mut m: Match = env
            .storage()
            .persistent()
            .get(&DataKey::Match(match_id))
            .ok_or(Error::MatchNotFound)?;

        if m.state == MatchState::Cancelled {
            return Err(Error::MatchCancelled);
        }
        if m.state == MatchState::Completed {
            return Err(Error::MatchCompleted);
        }
        if m.state != MatchState::Pending {
            return Err(Error::InvalidState);
        }

        let is_p1 = player == m.player1;
        let is_p2 = player == m.player2;

        if !is_p1 && !is_p2 {
            return Err(Error::Unauthorized);
        }

        let already_deposited = if is_p1 {
            m.player1_deposited
        } else {
            m.player2_deposited
        };

        if already_deposited {
            return Err(Error::AlreadyFunded);
        }

        let client = token::Client::new(&env, &m.token);
        let allowance = client.allowance(&player, &env.current_contract_address());
        if allowance < m.stake_amount {
            return Err(Error::InsufficientAllowance);
        }
        client
            .try_transfer(&player, &env.current_contract_address(), &m.stake_amount)
            .map_err(|_| Error::TransferFailed)?
            .map_err(|_| Error::TransferFailed)?;

        if is_p1 {
            m.player1_deposited = true;
        } else {
            m.player2_deposited = true;
        }

        // Check if exactly one player has now deposited (half-funded state)
        let one_deposited = (m.player1_deposited as u8) + (m.player2_deposited as u8) == 1;

        if one_deposited {
            // STATE TRANSITION: Pending → Half-Funded (one player deposited)
            // Emit event to signal this observability milestone
            let player_label = if is_p1 {
                symbol_short!("player1")
            } else {
                symbol_short!("player2")
            };
            env.events().publish(
                (Symbol::new(&env, "match"), symbol_short!("half_fun")),
                (match_id, player_label, m.stake_amount),
            );
        }

        if m.player1_deposited && m.player2_deposited {
            // STATE TRANSITION: Pending → Active
            // Record the ledger at which the match became active for timeout tracking.
            m.state = MatchState::Active;
            m.activated_ledger = Some(env.ledger().sequence());
            env.events().publish(
                (Symbol::new(&env, "match"), symbol_short!("activated")),
                match_id,
            );
        } else {
            env.events().publish(
                (Symbol::new(&env, "match"), symbol_short!("half_fun")),
                (
                    match_id,
                    player.clone(),
                    m.stake_amount,
                    player_label.clone(),
                ),
            );
        }

        env.events().publish(
            (Symbol::new(&env, "match"), symbol_short!("deposit")),
            (match_id, player, m.stake_amount, player_label),
        );

        env.storage()
            .persistent()
            .set(&DataKey::Match(match_id), &m);
        env.storage().persistent().extend_ttl(
            &DataKey::Match(match_id),
            MATCH_TTL_LEDGERS,
            MATCH_TTL_LEDGERS,
        );
        Self::bump_instance_ttl(&env);

        Ok(())
    }

    /// Oracle submits the verified match result. Transitions match to `PendingResult`
    /// and starts the dispute window. Payout is NOT executed immediately — call
    /// `finalize_result` after `DISPUTE_WINDOW_LEDGERS` to execute the payout.
    ///
    /// `game_id` must match the game_id stored in the match to prevent cross-match
    /// result injection.
    pub fn submit_result(
        env: Env,
        match_id: u64,
        game_id: String,
        winner: Winner,
        caller: Address,
    ) -> Result<(), Error> {
        if Self::is_paused(&env) {
            return Err(Error::ContractPaused);
        }

        let oracle: Address = env
            .storage()
            .instance()
            .get(&DataKey::Oracle)
            .ok_or(Error::Unauthorized)?;

        if caller != oracle {
            return Err(Error::Unauthorized);
        }
        caller.require_auth();

        Self::validate_match_id(&env, match_id)?;

        let mut m: Match = env
            .storage()
            .persistent()
            .get(&DataKey::Match(match_id))
            .ok_or(Error::MatchNotFound)?;

        if m.game_id != game_id {
            return Err(Error::GameIdMismatch);
        }

        if m.state != MatchState::Active {
            return Err(Error::InvalidState);
        }

        if !m.player1_deposited || !m.player2_deposited {
            // Invariant: Active state implies both players have deposited.
            // This check is a defensive guard — it should never be reached because
            // the match can only transition to Active once both deposits are confirmed.
            // It is retained to prevent any future refactor from inadvertently allowing
            // a payout on a match where funds were never fully escrowed.
            return Err(Error::NotFunded);
        }

        // STATE TRANSITION: Active → PendingResult
        // The oracle's result enters a dispute window. No payout yet.
        m.state = MatchState::PendingResult;
        m.pending_result_ledger = Some(env.ledger().sequence());
        m.pending_winner = OptionalWinner::Some(winner.clone());

        env.storage()
            .persistent()
            .set(&DataKey::Match(match_id), &m);
        env.storage().persistent().extend_ttl(
            &DataKey::Match(match_id),
            MATCH_TTL_LEDGERS,
            MATCH_TTL_LEDGERS,
        );
        Self::bump_instance_ttl(&env);

        env.events().publish(
            (Symbol::new(&env, "oracle"), symbol_short!("pending")),
            (match_id, winner, m.pending_result_ledger),
        );

        Ok(())
    }

    /// Admin overrides an oracle result during the dispute window.
    ///
    /// Can only be called while the match is in `PendingResult` state and before
    /// `DISPUTE_WINDOW_LEDGERS` have elapsed since the result was submitted.
    ///
    /// # Errors
    ///
    /// * [`Error::Unauthorized`]      — caller is not the admin.
    /// * [`Error::InvalidState`]      — match is not in `PendingResult` state.
    /// * [`Error::DisputeWindowActive`] — dispute window has already expired; call
    ///   `finalize_result` instead.
    pub fn override_result(
        env: Env,
        match_id: u64,
        new_winner: Winner,
        caller: Address,
    ) -> Result<(), Error> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::Unauthorized)?;

        if caller != admin {
            return Err(Error::Unauthorized);
        }
        caller.require_auth();

        Self::validate_match_id(&env, match_id)?;

        let mut m: Match = env
            .storage()
            .persistent()
            .get(&DataKey::Match(match_id))
            .ok_or(Error::MatchNotFound)?;

        if m.state != MatchState::PendingResult {
            return Err(Error::InvalidState);
        }

        // Ensure the dispute window has not yet expired; after expiry the result
        // is final and must be processed via finalize_result.
        let prl = m
            .pending_result_ledger
            .ok_or(Error::InvalidState)?;
        let current = env.ledger().sequence();
        let dispute_window = Self::get_dispute_window_ledgers(&env);
        if current > prl + dispute_window {
            return Err(Error::DisputeWindowExpired);
        }

        let old_winner = m.pending_winner.clone();
        if old_winner == OptionalWinner::Some(new_winner.clone()) {
            return Ok(());
        }
        m.pending_winner = OptionalWinner::Some(new_winner.clone());

        env.storage()
            .persistent()
            .set(&DataKey::Match(match_id), &m);
        env.storage().persistent().extend_ttl(
            &DataKey::Match(match_id),
            MATCH_TTL_LEDGERS,
            MATCH_TTL_LEDGERS,
        );
        Self::bump_instance_ttl(&env);

        env.events().publish(
            (Symbol::new(&env, "oracle"), symbol_short!("overridn")),
            (match_id, old_winner, new_winner),
        );

        Ok(())
    }

    /// Finalize a pending result and execute payout after the dispute window has expired.
    ///
    /// May only be called by one of the **matched players** (`player1` or `player2`),
    /// the registered **oracle**, or the **admin**. This prevents a third party from
    /// triggering payout transactions on behalf of players (e.g. to collect fee rebates
    /// or grief a player who intended to call `finalize_result` themselves).
    ///
    /// # Arguments
    ///
    /// * `match_id` — The match to finalize.
    /// * `caller`   — Must be `player1`, `player2`, the oracle, or the admin; must authorize.
    ///
    /// # Errors
    ///
    /// * [`Error::Unauthorized`]        — caller is not a matched player, the oracle, or the admin.
    /// * [`Error::InvalidState`]        — match is not in `PendingResult` state.
    /// * [`Error::DisputeWindowActive`] — dispute window has not yet expired.
    pub fn finalize_result(env: Env, match_id: u64, caller: Address) -> Result<(), Error> {
        Self::validate_match_id(&env, match_id)?;

        let mut m: Match = env
            .storage()
            .persistent()
            .get(&DataKey::Match(match_id))
            .ok_or(Error::MatchNotFound)?;

        // Authorisation: only player1, player2, the oracle, or the admin may
        // call finalize_result. This stops any random account from triggering
        // the payout (e.g. to collect fee rebates or grief a player).
        let oracle: Address = env
            .storage()
            .instance()
            .get(&DataKey::Oracle)
            .ok_or(Error::Unauthorized)?;
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::Unauthorized)?;

        if caller != m.player1 && caller != m.player2 && caller != oracle && caller != admin {
            return Err(Error::Unauthorized);
        }
        caller.require_auth();

        if m.state != MatchState::PendingResult {
            return Err(Error::InvalidState);
        }

        let prl = m
            .pending_result_ledger
            .ok_or(Error::InvalidState)?;
        let current = env.ledger().sequence();
        let dispute_window = Self::get_dispute_window_ledgers(&env);
        if current <= prl + dispute_window {
            return Err(Error::DisputeWindowActive);
        }

        let winner = match m.pending_winner.clone() {
            OptionalWinner::Some(w) => w,
            OptionalWinner::None => return Err(Error::InvalidState),
        };

        let client = token::Client::new(&env, &m.token);

        let payout_amount: i128 = match &winner {
            Winner::Draw => m.stake_amount,
            _ => m.stake_amount * 2,
        };

        let total_payout = match &winner {
            Winner::Draw => payout_amount * 2,
            _ => payout_amount,
        };
        Self::ensure_reserve_for_payout(&env, &m.token, total_payout)?;

        match &winner {
            Winner::Player1 => {
                client.transfer(&env.current_contract_address(), &m.player1, &payout_amount)
            }
            Winner::Player2 => {
                client.transfer(&env.current_contract_address(), &m.player2, &payout_amount)
            }
            Winner::Draw => {
                client.transfer(&env.current_contract_address(), &m.player1, &payout_amount);
                client.transfer(&env.current_contract_address(), &m.player2, &payout_amount);
            }
        }

        // STATE TRANSITION: PendingResult → Completed
        m.state = MatchState::Completed;
        m.completed_ledger = Some(env.ledger().sequence());
        env.storage()
            .persistent()
            .set(&DataKey::Match(match_id), &m);
        env.storage().persistent().extend_ttl(
            &DataKey::Match(match_id),
            MATCH_TTL_LEDGERS,
            MATCH_TTL_LEDGERS,
        );
        Self::bump_instance_ttl(&env);

        env.events().publish(
            (Symbol::new(&env, "match"), symbol_short!("completed")),
            (match_id, winner, payout_amount),
        );

        Ok(())
    }

    /// Reclaim funds when the oracle never submits a result within `TIMEOUT_LEDGERS`.
    ///
    /// Either `player1` or `player2` may call this function if the match has been in the
    /// `Active` state for longer than `TIMEOUT_LEDGERS` (~7 days) without an oracle result.
    /// Both players receive their original `stake_amount` back. The match transitions to
    /// `Cancelled`.
    ///
    /// # Arguments
    ///
    /// * `match_id` — The match to reclaim funds from.
    /// * `caller`   — Must be `player1` or `player2`; must authorize the call.
    ///
    /// # Errors
    ///
    /// * [`Error::Unauthorized`]       — caller is neither player.
    /// * [`Error::InvalidState`]       — match is not `Active`.
    /// * [`Error::TimeoutNotReached`]  — `TIMEOUT_LEDGERS` have not yet elapsed since the
    ///                                   match became `Active`; it is too early to reclaim.
    pub fn claim_timeout(env: Env, match_id: u64, caller: Address) -> Result<(), Error> {
        caller.require_auth();

        Self::validate_match_id(&env, match_id)?;

        let mut m: Match = env
            .storage()
            .persistent()
            .get(&DataKey::Match(match_id))
            .ok_or(Error::MatchNotFound)?;

        if m.state != MatchState::Active {
            return Err(Error::InvalidState);
        }

        let is_player = caller == m.player1 || caller == m.player2;
        if !is_player {
            return Err(Error::Unauthorized);
        }

        let activated = m.activated_ledger.ok_or(Error::InvalidState)?;
        let current = env.ledger().sequence();
        let timeout = Self::get_timeout_ledgers(&env);
        if current <= activated + timeout {
            // Timeout period has not elapsed yet — reject with MatchTimedOut reused
            // as "too early". We return MatchTimedOut here to keep error codes minimal;
            // callers should interpret it as "timeout not yet reached".
            return Err(Error::MatchTimedOut);
        }

        let client = token::Client::new(&env, &m.token);
        let refund_total = m.stake_amount * 2;
        Self::ensure_reserve_for_payout(&env, &m.token, refund_total)?;
        // Refund both players their original stake
        client.transfer(&env.current_contract_address(), &m.player1, &m.stake_amount);
        client.transfer(&env.current_contract_address(), &m.player2, &m.stake_amount);

        // STATE TRANSITION: Active → Cancelled (via timeout)
        m.state = MatchState::Cancelled;
        m.cancelled_ledger = Some(env.ledger().sequence());
        env.storage()
            .persistent()
            .set(&DataKey::Match(match_id), &m);
        env.storage().persistent().extend_ttl(
            &DataKey::Match(match_id),
            MATCH_TTL_LEDGERS,
            MATCH_TTL_LEDGERS,
        );
        Self::bump_instance_ttl(&env);

        env.events().publish(
            (Symbol::new(&env, "match"), symbol_short!("timeout")),
            (match_id, caller),
        );

        Ok(())
    }

    /// Cancel a match and refund any deposits.
    ///
    /// Authorization model:
    /// - **Pending state** (neither or only one player deposited): the calling player's auth suffices.
    /// - **Pending state** (both players deposited, rare edge case): both players must authorize.
    /// - **Active state** (both players deposited, match is live): both `player1` and `player2`
    ///   must authorize, enabling a mutual cancel without waiting for the 7-day
    ///   `claim_timeout`. Both stakes are refunded immediately.
    ///
    /// Cancellation is allowed while the contract is paused so players can recover funds.
    pub fn cancel_match(env: Env, match_id: u64, caller: Address) -> Result<(), Error> {
        Self::validate_match_id(&env, match_id)?;

        let mut m: Match = env
            .storage()
            .persistent()
            .get(&DataKey::Match(match_id))
            .ok_or(Error::MatchNotFound)?;

        // Allow cancellation from Pending or Active state only.
        // Completed and Cancelled are terminal — no further transitions.
        if m.state != MatchState::Pending && m.state != MatchState::Active {
            return Err(Error::InvalidState);
        }

        let is_p1 = caller == m.player1;
        let is_p2 = caller == m.player2;

        if !is_p1 && !is_p2 {
            return Err(Error::Unauthorized);
        }

        if m.state == MatchState::Active {
            // Active state: both players must mutually agree to cancel.
            // Require authorization from both parties since both have already
            // committed funds and neither should be able to unilaterally cancel.
            m.player1.require_auth();
            m.player2.require_auth();
        } else if m.player1_deposited && m.player2_deposited {
            // Pending state but both deposited (rare): same mutual-auth requirement.
            m.player1.require_auth();
            m.player2.require_auth();
        } else {
            caller.require_auth();
        }

        let client = token::Client::new(&env, &m.token);
        let player1_refund: i128 = if m.player1_deposited { m.stake_amount } else { 0 };
        let player2_refund: i128 = if m.player2_deposited { m.stake_amount } else { 0 };
        let refund_total = player1_refund + player2_refund;
        if refund_total > 0 {
            Self::ensure_reserve_for_payout(&env, &m.token, refund_total)?;
        }
        if m.player1_deposited {
            client
                .try_transfer(&env.current_contract_address(), &m.player1, &m.stake_amount)
                .map_err(|_| Error::TransferFailed)?
                .map_err(|_| Error::TransferFailed)?;
        }
        if m.player2_deposited {
            client
                .try_transfer(&env.current_contract_address(), &m.player2, &m.stake_amount)
                .map_err(|_| Error::TransferFailed)?
                .map_err(|_| Error::TransferFailed)?;
        }

        // STATE TRANSITION: Pending → Cancelled
        m.state = MatchState::Cancelled;
        m.cancelled_ledger = Some(env.ledger().sequence());
        env.storage()
            .persistent()
            .set(&DataKey::Match(match_id), &m);
        env.storage().persistent().extend_ttl(
            &DataKey::Match(match_id),
            MATCH_TTL_LEDGERS,
            MATCH_TTL_LEDGERS,
        );
        Self::bump_instance_ttl(&env);

        env.events().publish(
            (Symbol::new(&env, "match"), symbol_short!("cancelled")),
            (match_id, caller, player1_refund, player2_refund),
        );

        Ok(())
    }

    /// Drain all token holdings to the pre-registered safe address — admin only,
    /// requires contract to be paused.
    ///
    /// The destination is fixed at initialization time via the `safe_address`
    /// parameter of [`initialize`](EscrowContract::initialize) and stored under
    /// [`DataKey::SafeAddress`]. The `to` parameter has been intentionally removed:
    /// accepting an arbitrary destination address would allow a compromised or
    /// malicious admin to redirect the drain to any address, constituting a
    /// rug-pull vector against all players with funds in escrow.
    ///
    /// # Errors
    ///
    /// * [`Error::Unauthorized`] — `caller` is not the admin.
    /// * [`Error::NotPaused`]    — the contract is not currently paused.
    pub fn emergency_drain(env: Env, caller: Address) -> Result<(), Error> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::Unauthorized)?;

        if caller != admin {
            return Err(Error::Unauthorized);
        }
        caller.require_auth();

        if !Self::is_paused(&env) {
            return Err(Error::NotPaused);
        }

        // Safety guard: block the drain when any match is currently `Active`
        // (both players have deposited and the game is in progress). Draining
        // while funds are locked for active matches would silently steal player
        // stakes. The admin must wait for every active match to reach a terminal
        // state — `Completed` (payout done) or `Cancelled` (funds refunded) —
        // before calling emergency_drain. (#8)
        let match_count = Self::get_match_count(&env);
        let mut i: u64 = 0;
        while i < match_count {
            if let Some(m) = env
                .storage()
                .persistent()
                .get::<DataKey, Match>(&DataKey::Match(i))
            {
                if m.state == MatchState::Active {
                    return Err(Error::ActiveMatchExists);
                }
            }
            i += 1;
        }

        let safe_address: Address = env
            .storage()
            .instance()
            .get(&DataKey::SafeAddress)
            .ok_or(Error::Unauthorized)?;

        let token: Address = env
            .storage()
            .instance()
            .get(&DataKey::Token)
            .ok_or(Error::Unauthorized)?;

        let client = token::Client::new(&env, &token);
        let contract = env.current_contract_address();
        let balance = client.balance(&contract);

        if balance > 0 {
            client.transfer(&contract, &safe_address, &balance);
            env.events().publish(
                (Symbol::new(&env, "admin"), symbol_short!("drain")),
                (balance, safe_address, admin),
            );
        } else {
            // Preserve the audit trail even when there are no funds to drain.
            // A silent success with no event would complicate post-mortems.
            env.events().publish(
                (Symbol::new(&env, "admin"), symbol_short!("drn_noop")),
                (0i128, safe_address, admin),
            );
        }
        Self::bump_instance_ttl(&env);

        Ok(())
    }

    /// Return the match_id that registered a given game_id, or None if unregistered.
    pub fn get_game_id_owner(env: Env, game_id: String) -> Option<u64> {
        env.storage()
            .persistent()
            .get(&DataKey::GameId(game_id))
    }

    /// Return the token address this contract was initialized with.
    ///
    /// Allows frontends and integrators to verify which SEP-41 token a deployed
    /// contract accepts without parsing raw WASM storage.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Unauthorized`] if the contract has not been initialized yet.
    pub fn get_token(env: Env) -> Result<Address, Error> {
        env.storage()
            .instance()
            .get(&DataKey::Token)
            .ok_or(Error::Unauthorized)
    }

    /// Read a match by ID.
    pub fn get_match(env: Env, match_id: u64) -> Result<Match, Error> {
        Self::validate_match_id(&env, match_id)?;
        env.storage()
            .persistent()
            .get(&DataKey::Match(match_id))
            .ok_or(Error::MatchNotFound)
    }

    /// Read the platform of a match by ID.
    pub fn get_platform(env: Env, match_id: u64) -> Result<Platform, Error> {
        Self::validate_match_id(&env, match_id)?;
        let m: Match = env
            .storage()
            .persistent()
            .get(&DataKey::Match(match_id))
            .ok_or(Error::MatchNotFound)?;
        Ok(m.platform)
    }

    /// Check whether both players have deposited.
    ///
    /// Returns `Error::MatchNotFound` when `match_id` does not correspond to any
    /// existing match, rather than panicking. This covers both a `match_id` that
    /// was never created and one that exceeds the current match counter.
    pub fn is_funded(env: Env, match_id: u64) -> Result<bool, Error> {
        Self::validate_match_id(&env, match_id)?;
        let m: Match = env
            .storage()
            .persistent()
            .get(&DataKey::Match(match_id))
            .ok_or(Error::MatchNotFound)?;
        Ok(m.player1_deposited && m.player2_deposited)
    }

    /// Return the total escrowed balance for a match (0, 1x, or 2x stake).
    pub fn get_escrow_balance(env: Env, match_id: u64) -> Result<i128, Error> {
        let m: Match = env
            .storage()
            .persistent()
            .get(&DataKey::Match(match_id))
            .ok_or(Error::MatchNotFound)?;
        if m.state == MatchState::Completed || m.state == MatchState::Cancelled {
            return Ok(0);
        }
        let deposited: i128 = match (m.player1_deposited, m.player2_deposited) {
            (true, true) => 2,
            (true, false) | (false, true) => 1,
            (false, false) => 0,
        };
        Ok(deposited * m.stake_amount)
    }

    /// Return a page of match IDs in the range `[start, start + limit)`.
    ///
    /// # Offset-Based Pagination
    ///
    /// This function uses **offset-based pagination**, returning consecutive match IDs
    /// starting from `start`. It is suitable for dense ID spaces where matches are
    /// created frequently and ID gaps are rare.
    ///
    /// # Parameters
    ///
    /// * `start` — The first match ID to include in the result (inclusive).
    /// * `limit` — Maximum number of IDs to return. Capped at 100 to prevent unbounded queries.
    ///
    /// # Returns
    ///
    /// A `Vec<u64>` of match IDs in ascending order. May contain fewer than `limit` IDs if:
    /// - The query reaches the current match count (end of data), or
    /// - The remaining space in the vector is exhausted.
    ///
    /// # End-of-Data Detection
    ///
    /// Callers can detect the end of the full ID space when the returned vector has
    /// **fewer than `limit` entries**. This indicates no more matches exist beyond
    /// `start + returned_count`.
    ///
    /// **Important**: If matches are sparse (e.g., many were cancelled), a shorter result
    /// **does not guarantee end-of-data** — there may be gaps in the ID sequence. Use
    /// [`list_matches_after`](EscrowContract::list_matches_after) for cursor-based pagination
    /// if you need to iterate over a sparse ID space.
    ///
    /// # Examples
    ///
    /// Iterate through all matches starting from ID 0:
    /// ```text
    /// let mut start = 0;
    /// loop {
    ///     let page = contract.list_matches(start, 50);
    ///     if page.is_empty() {
    ///         break; // No more matches
    ///     }
    ///     // Process page...
    ///     if page.len() < 50 {
    ///         break; // Last page reached
    ///     }
    ///     start = page.last().unwrap() + 1;
    /// }
    /// ```
    ///
    /// # Sparse ID Spaces
    ///
    /// If match creation is infrequent or many matches were cancelled, the ID space
    /// becomes sparse. In this case, offset-based pagination may return short pages
    /// even when more matches exist. Use [`list_matches_after`](EscrowContract::list_matches_after)
    /// instead for robust pagination over sparse spaces.
    pub fn list_matches(env: Env, start: u64, limit: u32) -> Vec<u64> {
        const MAX_LIMIT: u32 = 100;
        let limit = limit.min(MAX_LIMIT);
        let count = Self::get_match_count(&env);
        let mut ids: Vec<u64> = vec![&env];
        let end = start.saturating_add(limit as u64).min(count);
        let mut i = start;
        while i < end {
            ids.push_back(i);
            i += 1;
        }
        ids
    }

    /// Return match IDs following a cursor position, using cursor-based pagination.
    ///
    /// # Cursor-Based Pagination
    ///
    /// This function returns consecutive match IDs **after** a given `after_match_id`.
    /// It is designed for **sparse ID spaces** where gaps are common due to cancelled
    /// or failed matches. Unlike offset-based pagination, cursor-based pagination
    /// guarantees that callers can always distinguish "no more data" from "data gap."
    ///
    /// # Parameters
    ///
    /// * `after_match_id` — The cursor position. Results will start from the next valid
    ///   match ID after this value. Pass `u64::MAX` to start from the beginning (finds ID 0).
    /// * `limit` — Maximum number of IDs to return. Capped at 100 to prevent unbounded queries.
    ///
    /// # Returns
    ///
    /// A `Vec<u64>` of match IDs in ascending order, all with `id > after_match_id`.
    /// May contain fewer than `limit` IDs if the current match count is reached.
    ///
    /// # End-of-Data Detection
    ///
    /// The end of the ID space is reached when the returned vector is **empty**.
    /// This unambiguously means: "There are no valid match IDs greater than `after_match_id`."
    ///
    /// # Examples
    ///
    /// Iterate through all matches using cursor-based pagination:
    /// ```text
    /// let mut cursor = u64::MAX; // Start from the beginning
    /// loop {
    ///     let page = contract.list_matches_after(cursor, 50);
    ///     if page.is_empty() {
    ///         break; // No more matches
    ///     }
    ///     // Process page...
    ///     cursor = page.last().unwrap(); // Move cursor to the last ID
    /// }
    /// ```
    ///
    /// # Advantages Over Offset-Based Pagination
    ///
    /// - **Unambiguous end-of-data**: Empty result always means end-of-data (no false "gaps").
    /// - **Handles sparse IDs**: Works correctly even if many matches were cancelled.
    /// - **Stable iteration**: Adding new matches does not affect the pagination of earlier pages.
    /// - **Cursor reuse**: A cursor remains valid even if the contract state changes between calls.
    pub fn list_matches_after(env: Env, after_match_id: u64, limit: u32) -> Vec<u64> {
        const MAX_LIMIT: u32 = 100;
        let limit = limit.min(MAX_LIMIT);
        let count = Self::get_match_count(&env);
        let mut ids: Vec<u64> = vec![&env];

        // Start from after_match_id + 1, capped at count
        let start = after_match_id.saturating_add(1).min(count);
        let end = start.saturating_add(limit as u64).min(count);

        let mut i = start;
        while i < end {
            ids.push_back(i);
            i += 1;
        }
        ids
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod tests_e2e;

#[cfg(test)]
mod tests_fuzz;
