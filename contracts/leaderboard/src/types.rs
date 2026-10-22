use soroban_sdk::{contracterror, contracttype, Address};

/// Errors returned by the leaderboard contract.
///
/// | Code | Variant          | Meaning                                        |
/// |------|------------------|------------------------------------------------|
/// |  1   | AlreadyInitialized | `initialize` called more than once           |
/// |  2   | Unauthorized     | Caller is not registered as a data source     |
/// |  3   | NotInitialized   | Contract has not been initialized yet          |
/// |  4   | InvalidResult    | Result is not a recognised variant             |
/// |  5   | InvalidRange     | `limit` is zero or the page offset is past the end |
/// |  6   | ContractPaused   | Result recording is paused                     |
/// |  7   | Overflow         | A leaderboard aggregate exceeds its integer range |
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Error {
    /// [E001] The contract has already been initialized.
    AlreadyInitialized = 1,

    /// [E002] The caller is not a registered data source.
    Unauthorized = 2,

    /// [E003] The contract has not been initialized.
    NotInitialized = 3,

    /// [E004] The supplied result is not a recognised variant.
    InvalidResult = 4,

    /// [E005] `limit` was zero, or `offset` was past the end of the results.
    InvalidRange = 5,

    /// [E006] Result recording is paused.
    ContractPaused = 6,

    /// [E007] A leaderboard aggregate exceeds its integer range.
    Overflow = 7,
}

/// The outcome of a single match, as reported by the escrow contract.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MatchOutcome {
    /// Player 1 won the match.
    Player1Won,
    /// Player 2 won the match.
    Player2Won,
    /// The match ended in a draw; both players had their stake returned.
    Draw,
}

/// A player's lifetime record.
///
/// `wins + losses + draws` is the number of completed matches that player has
/// taken part in. `earnings` is in the token's smallest unit and is therefore
/// **not** comparable across tokens — a player who only staked USDC and a
/// player who only staked XLM produce numbers on different scales. The
/// leaderboard surfaces the token alongside the figure for this reason.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlayerStats {
    /// Completed matches won.
    pub wins: u32,
    /// Completed matches lost.
    pub losses: u32,
    /// Completed matches drawn.
    pub draws: u32,
    /// Total amount won, in the token's smallest unit.
    pub earnings: i128,
    /// Address of the token `earnings` is denominated in.
    pub token: Address,
}

impl PlayerStats {
    /// A zeroed record for a player who has not been seen.
    ///
    /// Written by hand rather than derived from `Default` because `Address` has
    /// no `Default` impl. Returning a zeroed record (rather than `None`) keeps
    /// `get_stats` total — a caller never has to handle an absent player.
    pub fn empty(token: &Address) -> Self {
        PlayerStats {
            wins: 0,
            losses: 0,
            draws: 0,
            earnings: 0,
            token: token.clone(),
        }
    }

    /// Total completed matches this player took part in.
    pub fn matches_played(&self) -> Option<u32> {
        self.wins
            .checked_add(self.losses)?
            .checked_add(self.draws)
    }
}

/// One row of [`get_leaderboard`](crate::LeaderboardContract::get_leaderboard).
///
/// The player address is carried in the row rather than looked up separately so
/// a caller rendering a page does not need one RPC round-trip per row.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LeaderboardEntry {
    /// The player's Stellar address.
    pub player: Address,
    /// The player's lifetime record.
    pub stats: PlayerStats,
}

/// Storage keys.
///
/// | Key                  | Tier       | Contents                              |
/// |----------------------|------------|---------------------------------------|
/// | `Admin`              | Instance   | Address permitted to pause and re-key |
/// | `Paused`             | Instance   | Whether recording results is blocked  |
/// | `Stats(Address)`     | Persistent | [`PlayerStats`] for one address       |
/// | `ResultCount`        | Instance   | Number of results recorded so far     |
/// | `Sources(Address)`   | Instance   | Whether an address may submit results |
/// | `Roster`             | Instance   | Every address that has a stats entry  |
#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    /// The administrator.
    Admin,
    /// Whether result recording is paused.
    Paused,
    /// A player's [`PlayerStats`].
    Stats(Address),
    /// How many results have been recorded, used to bound the leaderboard.
    ResultCount,
    /// Settlement identifiers already processed to prevent duplicate relayer submissions.
    SettlementIds,
    /// Whether `Address` is authorised to submit results.
    Sources(Address),
    /// Every address that has a [`PlayerStats`] entry.
    ///
    /// Persistent storage cannot be enumerated on-chain, so `get_leaderboard`
    /// has no other way to discover which players exist. This is the index that
    /// makes the leaderboard readable.
    Roster,
}

/// Maximum number of rows a single `get_leaderboard` call may return.
///
/// A read that returns an unbounded `Vec` lets one caller force a read that
/// costs more as the contract grows, and the response must fit in a single
/// transaction's return budget. Callers page with `offset` instead.
pub const MAX_PAGE_SIZE: u32 = 50;

/// How many ledgers a [`PlayerStats`] entry lives once written.
///
/// Matches the escrow's match retention so a player's record outlives every
/// match that contributes to it. A leaderboard that expires before its matches
/// would report fewer games than actually happened.
pub const STATS_TTL_LEDGERS: u32 = 518_400;
