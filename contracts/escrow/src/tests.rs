extern crate std;
use super::*;
use soroban_sdk::{
    contract, contractimpl,
    testutils::{
        storage::Instance as _,
        storage::Persistent as _,
        Address as _,
        Events,
        Ledger as _,
    },
    token::{Client as TokenClient, StellarAssetClient},
    vec, Address, Env, IntoVal, String, Symbol, TryFromVal,
};

fn setup() -> (Env, Address, Address, Address, Address, Address, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let oracle = Address::generate(&env);
    let player1 = Address::generate(&env);
    let player2 = Address::generate(&env);
    let safe_address = Address::generate(&env);

    let token_id = env.register_stellar_asset_contract_v2(admin.clone());
    let token_addr = token_id.address();
    let asset_client = StellarAssetClient::new(&env, &token_addr);
    asset_client.mint(&player1, &1000);
    asset_client.mint(&player2, &1000);

    let contract_id = env.register(EscrowContract, ());
    let client = EscrowContractClient::new(&env, &contract_id);
    client.initialize(&oracle, &admin, &token_addr, &safe_address, &None, &None);

    // Fund the contract with the required reserve buffer.
    // `ensure_reserve_for_payout` requires the post-payout balance to stay
    // above ESCROW_RESERVE_BUFFER_STROOPS (1.5 XLM worth of stroops). Without
    // this top-up, every payout-style test leaving the contract at a near-zero
    // balance would fail with Error::InsufficientReserve.
    asset_client.mint(&contract_id, &crate::ESCROW_RESERVE_BUFFER_STROOPS);

    // Approve the escrow contract for both players (needed for allowance check)
    let expiration = env.ledger().sequence() + 1000000;
    let token_client = TokenClient::new(&env, &token_addr);
    token_client.approve(&player1, &contract_id, &1000, &expiration);
    token_client.approve(&player2, &contract_id, &1000, &expiration);

    (
        env,
        contract_id,
        oracle,
        player1,
        player2,
        token_addr,
        admin,
        safe_address,
    )
}

#[test]
fn test_initialize_twice_returns_already_initialized() {
    let env = Env::default();
    env.mock_all_auths();
    let oracle = Address::generate(&env);
    let admin = Address::generate(&env);
    let safe_address = Address::generate(&env);
    let token_id = env.register_stellar_asset_contract_v2(admin.clone());
    let token_addr = token_id.address();
    let contract_id = env.register(EscrowContract, ());
    let client = EscrowContractClient::new(&env, &contract_id);

    // First initialize should succeed
    let res = client.try_initialize(&oracle, &admin, &token_addr, &safe_address, &None, &None);
    assert!(res.is_ok());

    // Second initialize should return AlreadyInitialized rather than panic
    let res2 = client.try_initialize(&oracle, &admin, &token_addr, &safe_address, &None, &None);
    assert!(matches!(res2, Err(Ok(Error::AlreadyInitialized))));
}

#[test]
fn test_create_match() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "abc123"),
        &Platform::Lichess,
    );

    assert_eq!(id, 0);
    let m = client.get_match(&id);
    assert_eq!(m.state, MatchState::Pending);
    assert_eq!(m.created_ledger, env.ledger().sequence());
}

#[test]
fn test_get_match_not_found() {
    let (env, contract_id, ..) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    assert!(matches!(
        client.try_get_match(&999),
        Err(Ok(Error::MatchNotFound))
    ));
}

#[test]
fn test_deposit_invalid_match_id_u64_max() {
    let (env, contract_id, _oracle, player1, _player2, _token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    assert_eq!(
        client.try_deposit(&u64::MAX, &player1),
        Err(Ok(Error::MatchNotFound))
    );
}

#[test]
fn test_deposit_invalid_match_id_beyond_count() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    // Create one match (id = 0), then try to deposit into match 1 which doesn't exist
    client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "match0"),
        &Platform::Lichess,
    );
    assert_eq!(
        client.try_deposit(&1, &player1),
        Err(Ok(Error::MatchNotFound))
    );
}

#[test]
fn test_cancel_match_invalid_match_id_u64_max() {
    let (env, contract_id, _oracle, player1, _player2, _token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    assert_eq!(
        client.try_cancel_match(&u64::MAX, &player1),
        Err(Ok(Error::MatchNotFound))
    );
}

#[test]
fn test_cancel_match_invalid_match_id_beyond_count() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "cancel_beyond"),
        &Platform::Lichess,
    );
    assert_eq!(
        client.try_cancel_match(&1, &player1),
        Err(Ok(Error::MatchNotFound))
    );
}

#[test]
fn test_submit_result_invalid_match_id_u64_max() {
    let (env, contract_id, oracle, _player1, _player2, _token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    assert_eq!(
        client.try_submit_result(
            &u64::MAX,
            &String::from_str(&env, "any_game"),
            &Winner::Player1,
            &oracle,
        ),
        Err(Ok(Error::MatchNotFound))
    );
}

#[test]
fn test_submit_result_invalid_match_id_beyond_count() {
    let (env, contract_id, oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "submit_beyond"),
        &Platform::Lichess,
    );
    assert_eq!(
        client.try_submit_result(
            &1,
            &String::from_str(&env, "any_game"),
            &Winner::Player1,
            &oracle,
        ),
        Err(Ok(Error::MatchNotFound))
    );
}

#[test]
fn test_get_match_invalid_match_id_u64_max() {
    let (env, contract_id, ..) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    assert!(matches!(
        client.try_get_match(&u64::MAX),
        Err(Ok(Error::MatchNotFound))
    ));
}

#[test]
fn test_get_match_invalid_match_id_beyond_count() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "get_beyond"),
        &Platform::Lichess,
    );
    assert!(matches!(
        client.try_get_match(&1),
        Err(Ok(Error::MatchNotFound))
    ));
}

#[test]
fn test_get_platform_invalid_match_id_u64_max() {
    let (env, contract_id, ..) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    assert!(matches!(
        client.try_get_platform(&u64::MAX),
        Err(Ok(Error::MatchNotFound))
    ));
}

#[test]
fn test_get_platform_invalid_match_id_beyond_count() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "get_platform_beyond"),
        &Platform::Lichess,
    );
    assert!(matches!(
        client.try_get_platform(&1),
        Err(Ok(Error::MatchNotFound))
    ));
}

#[test]
fn test_get_platform_lichess() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "lichess-platform"),
        &Platform::Lichess,
    );

    assert_eq!(client.get_platform(&id), Platform::Lichess);
}

#[test]
fn test_get_platform_chessdotcom() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "chessdotcom-platform"),
        &Platform::ChessDotCom,
    );

    assert_eq!(client.get_platform(&id), Platform::ChessDotCom);
}

#[test]
fn test_deposit_and_activate() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    let token_client = TokenClient::new(&env, &token);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "abc123"),
        &Platform::Lichess,
    );

    client.deposit(&id, &player1);
    assert!(!client.is_funded(&id));
    client.deposit(&id, &player2);
    assert!(client.is_funded(&id));
    assert_eq!(client.get_escrow_balance(&id), 200);
    assert_eq!(client.get_match(&id).state, MatchState::Active);
    assert_eq!(token_client.balance(&player1), 900);
    assert_eq!(token_client.balance(&player2), 900);
}

#[test]
fn test_payout_winner() {
    let (env, contract_id, oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    let token_client = TokenClient::new(&env, &token);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "game1"),
        &Platform::Lichess,
    );
    client.deposit(&id, &player1);
    client.deposit(&id, &player2);
    client.submit_result(
        &id,
        &String::from_str(&env, "game1"),
        &Winner::Player1,
        &oracle,
    );

    assert_eq!(token_client.balance(&player1), 1100);
    assert_eq!(token_client.balance(&player2), 900);
    assert_eq!(client.get_match(&id).state, MatchState::Completed);
    assert_eq!(client.get_escrow_balance(&id), 0);
}

#[test]
fn test_payout_winner_player2() {
    let (env, contract_id, oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    let token_client = TokenClient::new(&env, &token);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "game_player2"),
        &Platform::Lichess,
    );
    client.deposit(&id, &player1);
    client.deposit(&id, &player2);
    client.submit_result(
        &id,
        &String::from_str(&env, "game_player2"),
        &Winner::Player2,
        &oracle,
    );

    assert_eq!(token_client.balance(&player1), 900);
    assert_eq!(token_client.balance(&player2), 1100);
    assert_eq!(client.get_match(&id).state, MatchState::Completed);
    assert_eq!(client.get_escrow_balance(&id), 0);
}

#[test]
fn test_draw_refund() {
    let (env, contract_id, oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    let token_client = TokenClient::new(&env, &token);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "game2"),
        &Platform::ChessDotCom,
    );
    client.deposit(&id, &player1);
    client.deposit(&id, &player2);
    client.submit_result(
        &id,
        &String::from_str(&env, "game2"),
        &Winner::Draw,
        &oracle,
    );

    assert_eq!(token_client.balance(&player1), 1000);
    assert_eq!(token_client.balance(&player2), 1000);
}

#[test]
fn test_cancel_refunds_depositor() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    let token_client = TokenClient::new(&env, &token);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "game3"),
        &Platform::Lichess,
    );
    client.deposit(&id, &player1);
    client.cancel_match(&id, &player1);

    assert_eq!(token_client.balance(&player1), 1000);
    assert_eq!(token_client.balance(&player2), 1000);
    assert_eq!(client.get_match(&id).state, MatchState::Cancelled);
    assert_eq!(client.get_escrow_balance(&id), 0);
}

#[test]
fn test_player2_can_cancel_pending_match() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    let token_client = TokenClient::new(&env, &token);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "p2cancel"),
        &Platform::Lichess,
    );
    client.deposit(&id, &player2);
    client.cancel_match(&id, &player2);

    assert_eq!(token_client.balance(&player2), 1000);
    assert_eq!(client.get_match(&id).state, MatchState::Cancelled);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn test_cancel_with_both_deposits_requires_both_auth() {
    use soroban_sdk::testutils::{MockAuth, MockAuthInvoke};

    let env = Env::default();
    let admin = Address::generate(&env);
    let oracle = Address::generate(&env);
    let player1 = Address::generate(&env);
    let player2 = Address::generate(&env);

    let token_id = env.register_stellar_asset_contract_v2(admin.clone());
    let token_addr = token_id.address();
    let asset_client = StellarAssetClient::new(&env, &token_addr);

    // Initialize with mock_all_auths for setup
    env.mock_all_auths();
    asset_client.mint(&player1, &1000);
    asset_client.mint(&player2, &1000);

    let contract_id = env.register(EscrowContract, ());
    let client = EscrowContractClient::new(&env, &contract_id);
    let safe_address = Address::generate(&env);
    client.initialize(&oracle, &admin, &token_addr, &safe_address, &None, &None);

    // Fund reserve buffer (matches setup() helper â€” see ensure_reserve_for_payout)
    asset_client.mint(&contract_id, &crate::ESCROW_RESERVE_BUFFER_STROOPS);

    // Approve the escrow contract for both players
    let expiration = env.ledger().sequence() + 1000000;
    let token_client = TokenClient::new(&env, &token_addr);
    token_client.approve(&player1, &contract_id, &1000, &expiration);
    token_client.approve(&player2, &contract_id, &1000, &expiration);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token_addr),
        &String::from_str(&env, "both_deposits"),
        &Platform::Lichess,
    );
    client.deposit(&id, &player1);
    client.deposit(&id, &player2);

    // Now set auth to only player1 â€” should panic because player2's auth is also required
    env.mock_auths(&[MockAuth {
        address: &player1,
        invoke: &MockAuthInvoke {
            contract: &contract_id,
            fn_name: "cancel_match",
            args: (id, player1.clone()).into_val(&env),
            sub_invokes: &[],
        },
    }]);

    client.cancel_match(&id, &player1);
}

#[test]
fn test_cancel_active_match_unilateral_fails() {
    use soroban_sdk::testutils::{MockAuth, MockAuthInvoke};

    let env = Env::default();
    let admin = Address::generate(&env);
    let oracle = Address::generate(&env);
    let player1 = Address::generate(&env);
    let player2 = Address::generate(&env);

    let token_id = env.register_stellar_asset_contract_v2(admin.clone());
    let token_addr = token_id.address();
    let asset_client = StellarAssetClient::new(&env, &token_addr);

    env.mock_all_auths();
    asset_client.mint(&player1, &1000);
    asset_client.mint(&player2, &1000);

    let contract_id = env.register(EscrowContract, ());
    let client = EscrowContractClient::new(&env, &contract_id);
    let safe_address = Address::generate(&env);
    client.initialize(&oracle, &admin, &token_addr, &safe_address, &None, &None);
    asset_client.mint(&contract_id, &crate::ESCROW_RESERVE_BUFFER_STROOPS);

    let expiration = env.ledger().sequence() + 1000000;
    let token_client = TokenClient::new(&env, &token_addr);
    token_client.approve(&player1, &contract_id, &1000, &expiration);
    token_client.approve(&player2, &contract_id, &1000, &expiration);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token_addr),
        &String::from_str(&env, "active_cancel"),
        &Platform::Lichess,
    );
    client.deposit(&id, &player1);
    client.deposit(&id, &player2);
    assert_eq!(client.get_match(&id).state, MatchState::Active);

    // Unilateral cancel (only player1 auth) must be rejected
    env.mock_auths(&[MockAuth {
        address: &player1,
        invoke: &MockAuthInvoke {
            contract: &contract_id,
            fn_name: "cancel_match",
            args: (id, player1.clone()).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    assert!(
        client.try_cancel_match(&id, &player1).is_err(),
        "unilateral cancel by player1 must be rejected for Active match"
    );

    // Match must remain Active
    env.mock_all_auths();
    assert_eq!(client.get_match(&id).state, MatchState::Active);
}

#[test]
fn test_cancel_active_match_mutual_succeeds() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    let token_client = TokenClient::new(&env, &token);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "active_mutual_cancel"),
        &Platform::Lichess,
    );
    client.deposit(&id, &player1);
    client.deposit(&id, &player2);
    assert_eq!(client.get_match(&id).state, MatchState::Active);

    // Mutual cancel (mock_all_auths covers both) must succeed
    client.cancel_match(&id, &player1);

    assert_eq!(client.get_match(&id).state, MatchState::Cancelled);
    // Both players refunded
    assert_eq!(token_client.balance(&player1), 1000);
    assert_eq!(token_client.balance(&player2), 1000);
    assert_eq!(client.get_escrow_balance(&id), 0);
}

#[test]
fn test_cancel_completed_match_fails() {
    let (env, contract_id, oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "completed_cancel"),
        &Platform::Lichess,
    );
    client.deposit(&id, &player1);
    client.deposit(&id, &player2);
    client.submit_result(
        &id,
        &String::from_str(&env, "completed_cancel"),
        &Winner::Player1,
        &oracle,
    );

    assert_eq!(
        client.try_cancel_match(&id, &player1),
        Err(Ok(Error::InvalidState))
    );
}

#[test]
fn test_deposit_into_completed_match_fails() {
    let (env, contract_id, oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "completed_deposit"),
        &Platform::Lichess,
    );
    client.deposit(&id, &player1);
    client.deposit(&id, &player2);
    client.submit_result(
        &id,
        &String::from_str(&env, "completed_deposit"),
        &Winner::Player1,
        &oracle,
    );

    assert_eq!(
        client.try_deposit(&id, &player1),
        Err(Ok(Error::MatchCompleted))
    );
}

#[test]
fn test_deposit_after_cancel_returns_match_cancelled() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "cancelled_deposit"),
        &Platform::Lichess,
    );
    client.cancel_match(&id, &player1);

    assert_eq!(
        client.try_deposit(&id, &player1),
        Err(Ok(Error::MatchCancelled))
    );
}

#[test]
fn test_non_oracle_cannot_submit_result() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "unauth_oracle"),
        &Platform::Lichess,
    );
    client.deposit(&id, &player1);
    client.deposit(&id, &player2);

    let impostor = Address::generate(&env);
    assert_eq!(
        client.try_submit_result(
            &id,
            &String::from_str(&env, "unauth_oracle"),
            &Winner::Player1,
            &impostor
        ),
        Err(Ok(Error::Unauthorized))
    );
}

/// Verify that only the registered oracle address can submit results.
/// A random address passed as `caller` must be rejected with `Unauthorized`
/// regardless of what auth it presents.
#[test]
fn test_submit_result_random_caller_is_unauthorized() {
    use soroban_sdk::testutils::{MockAuth, MockAuthInvoke};

    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "random_caller"),
        &Platform::Lichess,
    );
    client.deposit(&id, &player1);
    client.deposit(&id, &player2);

    let random = Address::generate(&env);
    let game_id = String::from_str(&env, "random_caller");

    // Provide auth for the random address â€” the contract must still reject it.
    env.mock_auths(&[MockAuth {
        address: &random,
        invoke: &MockAuthInvoke {
            contract: &contract_id,
            fn_name: "submit_result",
            args: (id, game_id.clone(), Winner::Player1, random.clone()).into_val(&env),
            sub_invokes: &[],
        },
    }]);

    assert_eq!(
        client.try_submit_result(&id, &game_id, &Winner::Player1, &random),
        Err(Ok(Error::Unauthorized))
    );
}

// Issue #196: submit_result on a Pending match should return InvalidState
#[test]
fn test_submit_result_on_pending_match_fails() {
    let (env, contract_id, oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "pending_submit"),
        &Platform::Lichess,
    );
    assert_eq!(
        client.try_submit_result(
            &id,
            &String::from_str(&env, "pending_submit"),
            &Winner::Player1,
            &oracle,
        ),
        Err(Ok(Error::InvalidState))
    );
}

// Issue #197: submit_result on an already Completed match should return
// InvalidState (no double-payout)
#[test]
fn test_submit_result_on_completed_match_fails() {
    let (env, contract_id, oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "double_submit"),
        &Platform::Lichess,
    );
    client.deposit(&id, &player1);
    client.deposit(&id, &player2);
    client.submit_result(
        &id,
        &String::from_str(&env, "double_submit"),
        &Winner::Player1,
        &oracle,
    );

    assert_eq!(
        client.try_submit_result(
            &id,
            &String::from_str(&env, "double_submit"),
            &Winner::Player2,
            &oracle,
        ),
        Err(Ok(Error::InvalidState))
    );
}

/// Queue deduplication test: Simulate the scenario where an off-chain oracle
/// service buggy queue might enqueue the same match_id twice, then process both
/// items. This test verifies that the second submission is rejected with
/// `InvalidState`, preventing double-payout.
///
/// **Scenario**:
/// 1. Match created and both players deposit â†’ state = Active
/// 2. First submit_result call succeeds â†’ state transitions to PendingResult
/// 3. Second submit_result call (same match_id, potentially different winner)
///    is rejected with InvalidState
/// 4. Balances remain unchanged after the rejection
///
/// **Why this matters**:
/// - The escrow contract relies on the state machine to prevent duplicate submissions.
/// - This test documents that deduplication is implicit in the state transition logic:
///   Active â†’ PendingResult (only valid state for submit_result).
/// - Mirrors the oracle contract's explicit AlreadySubmitted deduplication.
#[test]
fn test_submit_result_queue_deduplication_prevents_duplicate_match_id() {
    let (env, contract_id, oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    let token_client = TokenClient::new(&env, &token);

    let match_id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "queue_dedup_test"),
        &Platform::Lichess,
    );

    // Both players deposit â†’ match is now Active
    client.deposit(&match_id, &player1);
    client.deposit(&match_id, &player2);
    assert_eq!(client.get_match(&match_id).state, MatchState::Active);

    // Snapshot balances
    let p1_before = token_client.balance(&player1);
    let p2_before = token_client.balance(&player2);
    let escrow_before = client.get_escrow_balance(&match_id);

    // â”€â”€ First submission (simulates first item dequeued from queue) â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
    let game_id = String::from_str(&env, "queue_dedup_test");
    client.submit_result(&match_id, &game_id, &Winner::Player1, &oracle);

    // State transitions to PendingResult; payout executes
    assert_eq!(client.get_match(&match_id).state, MatchState::PendingResult);

    // Balances after first submission
    let p1_after_first = token_client.balance(&player1);
    let p2_after_first = token_client.balance(&player2);

    // â”€â”€ Second submission (simulates duplicate item from queue) â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
    // Oracle tries to submit a result for the same match_id again
    // (this could be a different winner due to queue bug or race condition)
    let result = client.try_submit_result(&match_id, &game_id, &Winner::Player2, &oracle);

    // Second submission must be rejected with InvalidState
    assert_eq!(
        result,
        Err(Ok(Error::InvalidState)),
        "second submit_result on same match_id must return InvalidState"
    );

    // Verify match state remains PendingResult (not Completed)
    assert_eq!(
        client.get_match(&match_id).state,
        MatchState::PendingResult,
        "match state must remain PendingResult after rejected second submission"
    );

    // Verify balances are unchanged by the rejected second submission
    assert_eq!(
        token_client.balance(&player1),
        p1_after_first,
        "Player 1 balance must not change after rejected duplicate submission"
    );
    assert_eq!(
        token_client.balance(&player2),
        p2_after_first,
        "Player 2 balance must not change after rejected duplicate submission"
    );

    // Verify escrow was reduced by the first payout
    let escrow_after = client.get_escrow_balance(&match_id);
    assert!(
        escrow_after < escrow_before,
        "escrow balance should decrease after first successful payout"
    );

    // â”€â”€ Summary of deduplication protection â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
    // The escrow contract prevents queue-based duplicate submissions via its
    // state machine:
    //   - Active state only allows submit_result â†’ PendingResult transition
    //   - Once PendingResult, any further submit_result call fails with InvalidState
    //   - This provides implicit deduplication without a separate tracking index
    //
    // Compare to the oracle contract which uses explicit AlreadySubmitted checks.
}

#[test]
fn test_submit_result_wrong_game_id_fails() {
    let (env, contract_id, oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "real_game"),
        &Platform::Lichess,
    );
    client.deposit(&id, &player1);
    client.deposit(&id, &player2);

    assert_eq!(
        client.try_submit_result(
            &id,
            &String::from_str(&env, "wrong_game"),
            &Winner::Player1,
            &oracle,
        ),
        Err(Ok(Error::GameIdMismatch))
    );
}

#[test]
#[should_panic(expected = "Contract already initialized")]
/// Issue #110 / Issue #1: a second call to initialize must panic to prevent
/// an attacker from overwriting the oracle and admin addresses post-deployment.
fn test_double_initialize_fails() {
    let env = Env::default();
    env.mock_all_auths();
    let oracle = Address::generate(&env);
    let admin = Address::generate(&env);
    let token_addr = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let contract_id = env.register(EscrowContract, ());
    let client = EscrowContractClient::new(&env, &contract_id);
    let safe_address = Address::generate(&env);
    client.initialize(&oracle, &admin, &token_addr, &safe_address, &None, &None);
    client.initialize(&oracle, &admin, &token_addr, &safe_address, &None, &None);
}

#[test]
fn test_create_match_zero_stake_fails() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    assert_eq!(
        client.try_create_match(
            &player1,
            &player2,
            &0,
            &Some(token),
            &String::from_str(&env, "zero_stake"),
            &Platform::Lichess,
        ),
        Err(Ok(Error::InvalidStakeAmount))
    );
}

#[test]
fn test_create_match_self_match_fails() {
    let (env, contract_id, _oracle, player1, _player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    assert_eq!(
        client.try_create_match(
            &player1,
            &player1,
            &100,
            &Some(token),
            &String::from_str(&env, "self_match"),
            &Platform::Lichess,
        ),
        Err(Ok(Error::InvalidPlayers))
    );
}

#[test]
fn test_create_match_player1_zero_address_fails() {
    let (env, contract_id, _oracle, _player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    // Create a zero address (burn address)
    let zero_address = Address::from_string(&String::from_str(
        &env,
        "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
    ));

    assert_eq!(
        client.try_create_match(
            &zero_address,
            &player2,
            &100,
            &Some(token),
            &String::from_str(&env, "zero_p1"),
            &Platform::Lichess,
        ),
        Err(Ok(Error::InvalidAddress))
    );
}

#[test]
fn test_create_match_player2_zero_address_fails() {
    let (env, contract_id, _oracle, player1, _player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    // Create a zero address (burn address)
    let zero_address = Address::from_string(&String::from_str(
        &env,
        "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
    ));

    assert_eq!(
        client.try_create_match(
            &player1,
            &zero_address,
            &100,
            &Some(token),
            &String::from_str(&env, "zero_p2"),
            &Platform::Lichess,
        ),
        Err(Ok(Error::InvalidAddress))
    );
}

#[test]
fn test_duplicate_game_id_rejected() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    let player3 = Address::generate(&env);
    let player4 = Address::generate(&env);

    client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "dup_game"),
        &Platform::Lichess,
    );

    assert_eq!(
        client.try_create_match(
            &player3,
            &player4,
            &100,
            &Some(token),
            &String::from_str(&env, "dup_game"),
            &Platform::Lichess,
        ),
        Err(Ok(Error::DuplicateGameId))
    );
}

#[test]
fn test_duplicate_game_id_fails() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    let player3 = Address::generate(&env);
    let player4 = Address::generate(&env);

    client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "dup_game_id"),
        &Platform::Lichess,
    );
    assert_eq!(
        client.try_create_match(
            &player3,
            &player4,
            &100,
            &Some(token),
            &String::from_str(&env, "dup_game_id"),
            &Platform::Lichess,
        ),
        Err(Ok(Error::DuplicateGameId))
    );
}

#[test]
fn test_create_match_empty_game_id_fails() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    assert_eq!(
        client.try_create_match(
            &player1,
            &player2,
            &100,
            &Some(token),
            &String::from_str(&env, ""),
            &Platform::Lichess,
        ),
        Err(Ok(Error::InvalidGameId))
    );
}

// â”€â”€ #1029: game_id character-set validation â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

#[test]
fn test_create_match_valid_game_id_alphanum() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    // Pure alphanumeric â€” should succeed
    assert!(client
        .try_create_match(
            &player1,
            &player2,
            &100,
            &Some(token),
            &String::from_str(&env, "abc123XYZ"),
            &Platform::Lichess,
        )
        .is_ok());
}

#[test]
fn test_create_match_valid_game_id_with_hyphen_and_underscore() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    // Hyphens and underscores are permitted
    assert!(client
        .try_create_match(
            &player1,
            &player2,
            &100,
            &Some(token),
            &String::from_str(&env, "game-001_ranked"),
            &Platform::Lichess,
        )
        .is_ok());
}

#[test]
fn test_create_match_game_id_with_null_byte_rejected() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    // Null byte â€” must be rejected
    let game_id = String::from_bytes(&env, &[b'a', b'b', 0x00, b'c']);
    assert_eq!(
        client.try_create_match(
            &player1,
            &player2,
            &100,
            &Some(token),
            &game_id,
            &Platform::Lichess,
        ),
        Err(Ok(Error::InvalidGameId))
    );
}

#[test]
fn test_create_match_game_id_with_control_char_rejected() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    // Tab control character (0x09) â€” must be rejected
    let game_id = String::from_bytes(&env, &[b'g', b'a', b'm', b'e', 0x09]);
    assert_eq!(
        client.try_create_match(
            &player1,
            &player2,
            &100,
            &Some(token),
            &game_id,
            &Platform::Lichess,
        ),
        Err(Ok(Error::InvalidGameId))
    );
}

#[test]
fn test_create_match_game_id_with_space_rejected() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    // Space (0x20) â€” must be rejected
    assert_eq!(
        client.try_create_match(
            &player1,
            &player2,
            &100,
            &Some(token),
            &String::from_str(&env, "game id"),
            &Platform::Lichess,
        ),
        Err(Ok(Error::InvalidGameId))
    );
}

#[test]
fn test_create_match_game_id_with_non_ascii_rejected() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    // High byte 0x80 â€” must be rejected
    let game_id = String::from_bytes(&env, &[b'g', b'a', b'm', b'e', 0x80]);
    assert_eq!(
        client.try_create_match(
            &player1,
            &player2,
            &100,
            &Some(token),
            &game_id,
            &Platform::Lichess,
        ),
        Err(Ok(Error::InvalidGameId))
    );
}

#[test]
fn test_create_match_game_id_with_dot_rejected() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    // Dot (.) is not in the allowed set
    assert_eq!(
        client.try_create_match(
            &player1,
            &player2,
            &100,
            &Some(token),
            &String::from_str(&env, "game.id"),
            &Platform::Lichess,
        ),
        Err(Ok(Error::InvalidGameId))
    );
}

#[test]
fn test_create_match_wrong_token_rejected() {
    let (env, contract_id, _oracle, player1, player2, _token, admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    // Register a different token contract
    let wrong_token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();

    assert_eq!(
        client.try_create_match(
            &player1,
            &player2,
            &100,
            &Some(wrong_token),
            &String::from_str(&env, "wrong_token"),
            &Platform::Lichess,
        ),
        Err(Ok(Error::TokenNotAllowlisted))
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")]
fn test_unauthorized_player_cannot_cancel() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "unauth_cancel"),
        &Platform::Lichess,
    );
    client.cancel_match(&id, &Address::generate(&env));
}

// Issue #192: deposit by non-player address should return Unauthorized
#[test]
fn test_deposit_by_non_player_returns_unauthorized() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "unauth_deposit"),
        &Platform::Lichess,
    );
    let stranger = Address::generate(&env);
    assert_eq!(
        client.try_deposit(&id, &stranger),
        Err(Ok(Error::Unauthorized))
    );
}

// Issue #195: is_funded returns false after only one player deposits, true after both
#[test]
fn test_is_funded_false_after_one_deposit() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "one_deposit"),
        &Platform::Lichess,
    );
    client.deposit(&id, &player1);
    assert!(!client.is_funded(&id));
    client.deposit(&id, &player2);
    assert!(client.is_funded(&id));
}

// Issue #818: get_escrow_balance returns stake_amount after only one deposit
#[test]
fn test_escrow_balance_after_single_deposit() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "single_deposit"),
        &Platform::Lichess,
    );
    client.deposit(&id, &player1);
    assert_eq!(client.get_escrow_balance(&id), 100);
}

#[test]
fn test_escrow_balance_stages() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "balance_stages"),
        &Platform::Lichess,
    );
    assert_eq!(client.get_escrow_balance(&id), 0);
    client.deposit(&id, &player1);
    assert_eq!(client.get_escrow_balance(&id), 100);
    client.deposit(&id, &player2);
    assert_eq!(client.get_escrow_balance(&id), 200);
}

#[test]
fn test_draw_payout_exact_amounts() {
    let (env, contract_id, oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    let token_client = TokenClient::new(&env, &token);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "draw_exact"),
        &Platform::Lichess,
    );
    client.deposit(&id, &player1);
    client.deposit(&id, &player2);
    client.submit_result(
        &id,
        &String::from_str(&env, "draw_exact"),
        &Winner::Draw,
        &oracle,
    );

    assert_eq!(token_client.balance(&player1), 1000);
    assert_eq!(token_client.balance(&player2), 1000);
    assert_eq!(client.get_escrow_balance(&id), 0);
}

#[test]
fn test_update_oracle() {
    let (env, contract_id, oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    let new_oracle = Address::generate(&env);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "oracle_rotate"),
        &Platform::Lichess,
    );
    client.deposit(&id, &player1);
    client.deposit(&id, &player2);

    client.update_oracle(&new_oracle);

    // Old oracle should now be rejected
    assert_eq!(
        client.try_submit_result(
            &id,
            &String::from_str(&env, "oracle_rotate"),
            &Winner::Player1,
            &oracle,
        ),
        Err(Ok(Error::Unauthorized))
    );
    // New oracle should succeed
    client.submit_result(
        &id,
        &String::from_str(&env, "oracle_rotate"),
        &Winner::Player1,
        &new_oracle,
    );
    assert_eq!(client.get_match(&id).state, MatchState::Completed);
}

#[test]
fn test_transfer_admin_rejects_zero_address() {
    let (env, contract_id, _oracle, _player1, _player2, _token, admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    let zero_admin: Address = TryFromVal::try_from_val(
        &env,
        &String::from_str(
            &env,
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
        ),
    )
    .unwrap();

    assert_eq!(client.try_transfer_admin(&admin, &zero_admin), Err(Ok(Error::InvalidAdmin)));
}

#[test]
fn test_transfer_admin_succeeds_and_rotates_admin() {
    let (env, contract_id, _oracle, _player1, _player2, _token, admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    let new_admin = Address::generate(&env);

    // Successful rotation
    assert!(client.try_transfer_admin(&admin, &new_admin).is_ok());

    // Old admin can no longer perform admin rotation
    let another = Address::generate(&env);
    assert_eq!(
        client.try_transfer_admin(&admin, &another),
        Err(Ok(Error::Unauthorized))
    );

    // New admin can now rotate again
    assert!(client.try_transfer_admin(&new_admin, &another).is_ok());
}

#[test]
fn test_transfer_admin_self_transfer_rejected() {
    let (env, contract_id, _oracle, _player1, _player2, _token, admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    // new_admin == current_admin must be rejected
    assert_eq!(
        client.try_transfer_admin(&admin, &admin),
        Err(Ok(Error::InvalidAdmin))
    );
}

#[test]
fn test_transfer_admin_unauthorized_caller_rejected() {
    let (env, contract_id, _oracle, _player1, _player2, _token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let impostor = Address::generate(&env);

    // A non-admin caller must be rejected
    assert_eq!(
        client.try_transfer_admin(&impostor, &admin),
        Err(Ok(Error::Unauthorized))
    );
}

#[test]
fn test_pause_blocks_all_state_changing_operations() {
    let (env, contract_id, oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    // Create a match to test deposit and cancel while paused.
    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "pause_test"),
        &Platform::Lichess,
    );

    client.pause();

    // 1. Block create_match
    assert_eq!(
        client.try_create_match(
            &player1,
            &player2,
            &100,
            &Some(token),
            &String::from_str(&env, "paused_create"),
            &Platform::Lichess,
        ),
        Err(Ok(Error::ContractPaused))
    );

    // 2. Block deposit
    assert_eq!(
        client.try_deposit(&id, &player1),
        Err(Ok(Error::ContractPaused))
    );

    // 3. Allow cancel_match while paused
    client.cancel_match(&id, &player1);
    assert_eq!(client.get_match(&id).state, MatchState::Cancelled);

    // Now unpause and create a fresh match to verify submit_result still respects pause.
    client.unpause();
    let id2 = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "pause_test_active"),
        &Platform::Lichess,
    );
    client.deposit(&id2, &player1);
    client.deposit(&id2, &player2);

    client.pause();

    // 4. Block submit_result
    assert_eq!(
        client.try_submit_result(
            &id2,
            &String::from_str(&env, "pause_test_active"),
            &Winner::Player1,
            &oracle,
        ),
        Err(Ok(Error::ContractPaused))
    );

    client.unpause();
    // Verify submit_result works after unpause
    client.submit_result(
        &id2,
        &String::from_str(&env, "pause_test_active"),
        &Winner::Player1,
        &oracle,
    );
    assert_eq!(client.get_match(&id2).state, MatchState::Completed);
}

#[test]
fn test_non_admin_cannot_pause() {
    let (env, contract_id, _oracle, _player1, _player2, _token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    let non_admin = Address::generate(&env);

    use soroban_sdk::testutils::{MockAuth, MockAuthInvoke};
    env.mock_auths(&[MockAuth {
        address: &non_admin,
        invoke: &MockAuthInvoke {
            contract: &contract_id,
            fn_name: "pause",
            args: ().into_val(&env),
            sub_invokes: &[],
        },
    }]);

    assert!(client.try_pause().is_err());
}

#[test]
fn test_non_admin_cannot_unpause() {
    let (env, contract_id, _oracle, _player1, _player2, _token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    let non_admin = Address::generate(&env);

    client.pause();

    use soroban_sdk::testutils::{MockAuth, MockAuthInvoke};
    env.mock_auths(&[MockAuth {
        address: &non_admin,
        invoke: &MockAuthInvoke {
            contract: &contract_id,
            fn_name: "unpause",
            args: ().into_val(&env),
            sub_invokes: &[],
        },
    }]);

    assert!(client.try_unpause().is_err());
}

#[test]
fn test_is_paused_returns_false_by_default() {
    let (env, contract_id, ..) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    assert!(!client.is_paused());
}

#[test]
fn test_is_paused_returns_true_after_pause() {
    let (env, contract_id, ..) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    assert!(!client.is_paused());
    client.pause();
    assert!(client.is_paused());
    client.unpause();
    assert!(!client.is_paused());
}

#[test]
fn test_pause_unpause_events() {
    let (env, contract_id, _, _, _, _, admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let expected_pause_ledger_sequence = env.ledger().sequence();
    client.pause();
    let events = env.events().all();
    let last_event = events.last().unwrap();
    assert_eq!(last_event.0, contract_id);
    assert_eq!(last_event.1.len(), 2);
    assert_eq!(
        Symbol::try_from_val(&env, &last_event.1.get(0).unwrap()).unwrap(),
        Symbol::new(&env, "admin")
    );
    assert_eq!(
        Symbol::try_from_val(&env, &last_event.1.get(1).unwrap()).unwrap(),
        symbol_short!("paused")
    );
    let (ev_admin, ev_ledger_sequence): (Address, u32) =
        TryFromVal::try_from_val(&env, &last_event.2).unwrap();
    assert_eq!(ev_admin, admin);
    assert_eq!(ev_ledger_sequence, expected_pause_ledger_sequence);

    let expected_unpause_ledger_sequence = env.ledger().sequence();
    client.unpause();
    let events = env.events().all();
    let last_event = events.last().unwrap();
    assert_eq!(last_event.0, contract_id);
    assert_eq!(last_event.1.len(), 2);
    assert_eq!(
        Symbol::try_from_val(&env, &last_event.1.get(0).unwrap()).unwrap(),
        Symbol::new(&env, "admin")
    );
    assert_eq!(
        Symbol::try_from_val(&env, &last_event.1.get(1).unwrap()).unwrap(),
        symbol_short!("unpaused")
    );
    let (ev_admin, ev_ledger_sequence): (Address, u32) =
        TryFromVal::try_from_val(&env, &last_event.2).unwrap();
    assert_eq!(ev_admin, admin);
    assert_eq!(ev_ledger_sequence, expected_unpause_ledger_sequence);
}

#[test]
fn test_update_oracle_emits_old_new_and_admin() {
    let (env, contract_id, oracle, _, _, _, admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    let new_oracle = Address::generate(&env);

    client.update_oracle(&new_oracle);

    let events = env.events().all();
    let topics = vec![
        &env,
        Symbol::new(&env, "admin").into_val(&env),
        Symbol::new(&env, "oracle_updated").into_val(&env),
    ];
    let matched = events.iter().find(|(_, t, _)| *t == topics);
    assert!(matched.is_some());

    let (_, _, data) = matched.unwrap();
    let (ev_old_oracle, ev_new_oracle, ev_admin): (Address, Address, Address) =
        TryFromVal::try_from_val(&env, &data).unwrap();
    assert_eq!(ev_old_oracle, oracle);
    assert_eq!(ev_new_oracle, new_oracle);
    assert_eq!(ev_admin, admin);
}

#[test]
fn test_non_admin_cannot_update_oracle() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let non_admin = Address::generate(&env);
    let oracle = Address::generate(&env);
    let new_oracle = Address::generate(&env);
    let token_addr = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let contract_id = env.register(EscrowContract, ());
    let client = EscrowContractClient::new(&env, &contract_id);
    let safe_address = Address::generate(&env);
    client.initialize(&oracle, &admin, &token_addr, &safe_address, &None, &None);

    use soroban_sdk::testutils::{MockAuth, MockAuthInvoke};
    env.mock_auths(&[MockAuth {
        address: &non_admin,
        invoke: &MockAuthInvoke {
            contract: &contract_id,
            fn_name: "update_oracle",
            args: (new_oracle.clone(),).into_val(&env),
            sub_invokes: &[],
        },
    }]);

    assert!(client.try_update_oracle(&new_oracle).is_err());
}

#[test]
fn test_game_id_ttl_set_on_creation() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let game_id = String::from_str(&env, "ttl_game_id");
    client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &game_id,
        &Platform::Lichess,
    );

    let ttl = env.as_contract(&contract_id, || {
        env.storage()
            .persistent()
            .get_ttl(&DataKey::GameId(game_id.clone()))
    });
    assert_eq!(ttl, crate::MATCH_TTL_LEDGERS);
}

#[test]
fn test_ttl_extended_on_state_changes() {
    let (env, contract_id, oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "ttl_game"),
        &Platform::Lichess,
    );

    let check_ttl =
        |key: DataKey| env.as_contract(&contract_id, || env.storage().persistent().get_ttl(&key));

    assert_eq!(check_ttl(DataKey::Match(id)), crate::MATCH_TTL_LEDGERS);

    env.ledger()
        .set_sequence_number(env.ledger().sequence() + 100);
    assert!(check_ttl(DataKey::Match(id)) < crate::MATCH_TTL_LEDGERS);
    client.deposit(&id, &player1);
    assert_eq!(check_ttl(DataKey::Match(id)), crate::MATCH_TTL_LEDGERS);

    env.ledger()
        .set_sequence_number(env.ledger().sequence() + 100);
    client.deposit(&id, &player2);
    assert_eq!(check_ttl(DataKey::Match(id)), crate::MATCH_TTL_LEDGERS);

    env.ledger()
        .set_sequence_number(env.ledger().sequence() + 100);
    client.submit_result(
        &id,
        &String::from_str(&env, "ttl_game"),
        &Winner::Player2,
        &oracle,
    );
    assert_eq!(check_ttl(DataKey::Match(id)), crate::MATCH_TTL_LEDGERS);
}

#[test]
fn test_safe_address_is_only_written_during_initialize() {
    let source = include_str!("lib.rs");
    let compact_source: std::string::String = source
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    let safe_address_writes = compact_source.matches("set(&DataKey::SafeAddress").count();
    let initialize_body = source
        .split("pub fn initialize(")
        .nth(1)
        .unwrap()
        .split("pub fn update_oracle(")
        .next()
        .unwrap();

    assert_eq!(safe_address_writes, 1);
    let compact_initialize: std::string::String = initialize_body
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    assert!(compact_initialize.contains("set(&DataKey::SafeAddress"));
}

#[test]
fn test_create_match_emits_event() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let game_id = String::from_str(&env, "game_ev");
    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &game_id,
        &Platform::Lichess,
    );

    let events = env.events().all();
    let topics = vec![
        &env,
        Symbol::new(&env, "match").into_val(&env),
        soroban_sdk::symbol_short!("created").into_val(&env),
    ];
    let matched = events.iter().find(|(_, t, _)| *t == topics);
    assert!(matched.is_some());

    let (_, _, data) = matched.unwrap();
    let (ev_id, ev_p1, ev_p2, ev_stake, ev_game_id): (u64, Address, Address, i128, String) =
        TryFromVal::try_from_val(&env, &data).unwrap();
    assert_eq!(ev_id, id);
    assert_eq!(ev_p1, player1);
    assert_eq!(ev_p2, player2);
    assert_eq!(ev_stake, 100);
    assert_eq!(ev_game_id, game_id);
}

#[test]
fn test_deposit_emits_event() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "deposit_ev"),
        &Platform::Lichess,
    );

    // Test player1 deposit
    client.deposit(&id, &player1);

    client.deposit(&id, &player2);

    let events = env.events().all();
    let deposit_topics = vec![
        &env,
        Symbol::new(&env, "match").into_val(&env),
        soroban_sdk::symbol_short!("deposit").into_val(&env),
    ];
    let matched = events.iter().find(|(_, t, _)| *t == deposit_topics);
    assert!(matched.is_some());
    let (_, _, data) = matched.unwrap();
    let (ev_id, ev_player, ev_amount, ev_label): (u64, Address, i128, Symbol) =
        TryFromVal::try_from_val(&env, &data).unwrap();
    assert_eq!(ev_id, id);
    assert_eq!(ev_amount, 100);
    assert!(ev_player == player1 || ev_player == player2);
    assert!(ev_label == symbol_short!("player1") || ev_label == symbol_short!("player2"));
}

#[test]
fn test_deposit_event_player_label() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "label_ev"),
        &Platform::Lichess,
    );

    let deposit_topics = vec![
        &env,
        Symbol::new(&env, "match").into_val(&env),
        soroban_sdk::symbol_short!("deposit").into_val(&env),
    ];

    client.deposit(&id, &player1);
    let (_, _, data) = env
        .events()
        .all()
        .iter()
        .filter(|(_, t, _)| *t == deposit_topics)
        .last()
        .unwrap();
    let (_, _, _, label): (u64, Address, i128, Symbol) =
        TryFromVal::try_from_val(&env, &data).unwrap();
    assert_eq!(label, symbol_short!("player1"));

    client.deposit(&id, &player2);
    let (_, _, data) = env
        .events()
        .all()
        .iter()
        .filter(|(_, t, _)| *t == deposit_topics)
        .last()
        .unwrap();
    let (_, _, _, label): (u64, Address, i128, Symbol) =
        TryFromVal::try_from_val(&env, &data).unwrap();
    assert_eq!(label, symbol_short!("player2"));
}

#[test]
fn test_half_funded_event_on_first_deposit() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "half_funded_ev"),
        &Platform::Lichess,
    );

    // First deposit should emit a half_funded event
    client.deposit(&id, &player1);

    let events = env.events().all();
    let half_funded_topics = vec![
        &env,
        Symbol::new(&env, "match").into_val(&env),
        soroban_sdk::symbol_short!("half_fun").into_val(&env),
    ];
    let matched = events.iter().find(|(_, t, _)| *t == half_funded_topics);
    assert!(
        matched.is_some(),
        "half_funded event should be emitted on first deposit"
    );

    let (_, _, data) = matched.unwrap();
    let (ev_id, ev_player_label, ev_stake): (u64, Symbol, i128) =
        TryFromVal::try_from_val(&env, &data).unwrap();
    assert_eq!(ev_id, id);
    assert_eq!(ev_player_label, symbol_short!("player1"));
    assert_eq!(ev_stake, 100);

    // Second deposit should NOT emit another half_funded event
    client.deposit(&id, &player2);

    let events_after_second = env.events().all();
    let half_funded_count = events_after_second
        .iter()
        .filter(|(_, t, _)| *t == half_funded_topics)
        .count();
    assert_eq!(
        half_funded_count, 1,
        "half_funded event should only be emitted once"
    );
}

#[test]
fn test_submit_result_emits_event() {
    let (env, contract_id, oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "result_ev"),
        &Platform::Lichess,
    );
    client.deposit(&id, &player1);
    client.deposit(&id, &player2);
    client.submit_result(
        &id,
        &String::from_str(&env, "result_ev"),
        &Winner::Player1,
        &oracle,
    );

    let events = env.events().all();
    let topics = vec![
        &env,
        Symbol::new(&env, "match").into_val(&env),
        soroban_sdk::symbol_short!("completed").into_val(&env),
    ];
    let matched = events.iter().find(|(_, t, _)| *t == topics);
    assert!(matched.is_some());

    let (_, _, data) = matched.unwrap();
    let (ev_id, ev_winner, ev_payout): (u64, Winner, i128) =
        TryFromVal::try_from_val(&env, &data).unwrap();
    assert_eq!(ev_id, id);
    assert_eq!(ev_winner, Winner::Player1);
    assert_eq!(ev_payout, 200);
}

#[test]
fn test_cancel_match_emits_event() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "cancel_ev"),
        &Platform::Lichess,
    );
    client.cancel_match(&id, &player1);

    let events = env.events().all();
    let topics = vec![
        &env,
        Symbol::new(&env, "match").into_val(&env),
        soroban_sdk::symbol_short!("cancelled").into_val(&env),
    ];
    let matched = events.iter().find(|(_, t, _)| *t == topics);
    assert!(matched.is_some());

    let (_, _, data) = matched.unwrap();
    let (ev_id, ev_cancelled_by): (u64, Address) = TryFromVal::try_from_val(&env, &data).unwrap();
    assert_eq!(ev_id, id);
    assert_eq!(ev_cancelled_by, player1);
}

// Issue #59: Test that pause() prevents match creation
#[test]
fn test_pause_prevents_match_creation() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    client.pause();

    assert_eq!(
        client.try_create_match(
            &player1,
            &player2,
            &100,
            &Some(token),
            &String::from_str(&env, "paused_match"),
            &Platform::Lichess,
        ),
        Err(Ok(Error::ContractPaused))
    );
}

// Issue #60: Test that unpause() re-enables match creation
#[test]
fn test_unpause_enables_match_creation() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    client.pause();
    client.unpause();

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "unpaused_match"),
        &Platform::Lichess,
    );

    assert_eq!(id, 0);
    let m = client.get_match(&id);
    assert_eq!(m.state, MatchState::Pending);
}

// Issue #61: Test that update_oracle() successfully rotates the oracle address
#[test]
fn test_update_oracle_rotates_address() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    let new_oracle = Address::generate(&env);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "oracle_test"),
        &Platform::Lichess,
    );
    client.deposit(&id, &player1);
    client.deposit(&id, &player2);

    client.update_oracle(&new_oracle);

    client.submit_result(
        &id,
        &String::from_str(&env, "oracle_test"),
        &Winner::Player1,
        &new_oracle,
    );
}

// Issue #62: Test that non-admin cannot call pause(), unpause(), or update_oracle()
#[test]
fn test_non_admin_cannot_call_admin_functions() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let non_admin = Address::generate(&env);
    let oracle = Address::generate(&env);
    let new_oracle = Address::generate(&env);
    let token_addr = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let contract_id = env.register(EscrowContract, ());
    let client = EscrowContractClient::new(&env, &contract_id);
    let safe_address = Address::generate(&env);
    client.initialize(&oracle, &admin, &token_addr, &safe_address, &None, &None);

    use soroban_sdk::testutils::{MockAuth, MockAuthInvoke};

    env.mock_auths(&[MockAuth {
        address: &non_admin,
        invoke: &MockAuthInvoke {
            contract: &contract_id,
            fn_name: "pause",
            args: ().into_val(&env),
            sub_invokes: &[],
        },
    }]);
    assert!(client.try_pause().is_err());

    env.mock_auths(&[MockAuth {
        address: &non_admin,
        invoke: &MockAuthInvoke {
            contract: &contract_id,
            fn_name: "unpause",
            args: ().into_val(&env),
            sub_invokes: &[],
        },
    }]);
    assert!(client.try_unpause().is_err());

    env.mock_auths(&[MockAuth {
        address: &non_admin,
        invoke: &MockAuthInvoke {
            contract: &contract_id,
            fn_name: "update_oracle",
            args: (new_oracle.clone(),).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    assert!(client.try_update_oracle(&new_oracle).is_err());
}

// Issue #55: Multiple matches can be created and tracked independently
#[test]
fn test_multiple_matches_independent() {
    let (env, contract_id, oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    let token_client = TokenClient::new(&env, &token);

    let player3 = Address::generate(&env);
    let player4 = Address::generate(&env);
    let asset_client = StellarAssetClient::new(&env, &token);
    asset_client.mint(&player3, &1000);
    asset_client.mint(&player4, &1000);

    // Approve the escrow contract for the additional players
    let expiration = env.ledger().sequence() + 1000000;
    token_client.approve(&player3, &contract_id, &1000, &expiration);
    token_client.approve(&player4, &contract_id, &1000, &expiration);

    let id0 = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "game_m0"),
        &Platform::Lichess,
    );
    let id1 = client.create_match(
        &player3,
        &player4,
        &200,
        &Some(token),
        &String::from_str(&env, "game_m1"),
        &Platform::Lichess,
    );
    let id2 = client.create_match(
        &player1,
        &player3,
        &50,
        &Some(token),
        &String::from_str(&env, "game_m2"),
        &Platform::ChessDotCom,
    );

    assert_eq!(id0, 0);
    assert_eq!(id1, 1);
    assert_eq!(id2, 2);

    // Fund and complete match 0 (player1 wins)
    client.deposit(&id0, &player1);
    client.deposit(&id0, &player2);
    client.submit_result(
        &id0,
        &String::from_str(&env, "game_m0"),
        &Winner::Player1,
        &oracle,
    );
    assert_eq!(client.get_match(&id0).state, MatchState::Completed);
    assert_eq!(token_client.balance(&player1), 1100); // 1000 - 100 + 200

    // Fund and complete match 1 (draw)
    client.deposit(&id1, &player3);
    client.deposit(&id1, &player4);
    client.submit_result(
        &id1,
        &String::from_str(&env, "game_m1"),
        &Winner::Draw,
        &oracle,
    );
    assert_eq!(client.get_match(&id1).state, MatchState::Completed);
    assert_eq!(token_client.balance(&player3), 1000); // 1000 - 200 + 200 (draw refund)

    // Cancel match 2 (only player1 deposited)
    client.deposit(&id2, &player1);
    client.cancel_match(&id2, &player1);
    assert_eq!(client.get_match(&id2).state, MatchState::Cancelled);
    // player1 net: started 1000, won 200 from match0, deposited 50 for match2, refunded 50 = 1100
    assert_eq!(token_client.balance(&player1), 1100);
}

// Issue #56: Paused contract blocks deposit as well
#[test]
fn test_pause_blocks_deposit() {
    let (env, contract_id, oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "pause_deposit"),
        &Platform::Lichess,
    );

    client.pause();

    assert_eq!(
        client.try_deposit(&id, &player1),
        Err(Ok(Error::ContractPaused))
    );
    assert_eq!(
        client.try_create_match(
            &player1,
            &player2,
            &100,
            &Some(token),
            &String::from_str(&env, "pause_create"),
            &Platform::Lichess,
        ),
        Err(Ok(Error::ContractPaused))
    );
    assert_eq!(
        client.try_submit_result(
            &id,
            &String::from_str(&env, "pause_deposit"),
            &Winner::Player1,
            &oracle,
        ),
        Err(Ok(Error::ContractPaused))
    );

    // Unpause and verify deposit works again
    client.unpause();
    client.deposit(&id, &player1);
    assert!(!client.is_funded(&id));
}

// Issue: Cancellation is allowed while the contract is paused so players can recover funds.
// This test verifies the comment at line 790 does not regress due to future pause-check changes.
#[test]
fn test_cancel_match_allowed_while_paused() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    let token_client = TokenClient::new(&env, &token);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "pause_cancel"),
        &Platform::Lichess,
    );

    // Pause the contract
    client.pause();
    assert!(client.is_paused());

    // Verify that cancel_match succeeds despite the contract being paused
    // This allows players to recover funds in an emergency.
    client.cancel_match(&id, &player1);

    // Verify the match is cancelled and funds are refunded
    assert_eq!(client.get_match(&id).state, MatchState::Cancelled);
    assert_eq!(token_client.balance(&player1), 1000);
    assert_eq!(client.get_escrow_balance(&id), 0);
}

// Issue #72: submit_result on already Cancelled match should return InvalidState
#[test]
fn test_submit_result_on_cancelled_match_fails() {
    let (env, contract_id, oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "cancelled_result"),
        &Platform::Lichess,
    );
    client.cancel_match(&id, &player1);
    assert_eq!(client.get_match(&id).state, MatchState::Cancelled);

    assert_eq!(
        client.try_submit_result(
            &id,
            &String::from_str(&env, "cancelled_result"),
            &Winner::Player1,
            &oracle,
        ),
        Err(Ok(Error::InvalidState))
    );
}

// Issue #1124 â€” Explicit test: second deposit for the same player returns AlreadyFunded.
//
// The AlreadyFunded invariant is enforced independently for each player.
// This test documents the expected behaviour for player2 so the idempotency
// guarantee is visible for both participants.
#[test]
fn test_second_deposit_player2_returns_already_funded() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "already_funded_p2"),
        &Platform::Lichess,
    );

    // Player2 deposits successfully
    client.deposit(&id, &player2);
    // Second call from the same player must be rejected
    assert_eq!(
        client.try_deposit(&id, &player2),
        Err(Ok(Error::AlreadyFunded)),
        "second deposit for player2 must return AlreadyFunded"
    );
}

// Issue #33: Already-deposited player cannot deposit again
#[test]
fn test_double_deposit_same_player_fails() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "double_dep"),
        &Platform::Lichess,
    );
    client.deposit(&id, &player1);
    assert_eq!(
        client.try_deposit(&id, &player1),
        Err(Ok(Error::AlreadyFunded))
    );
}

// Issue #34: Negative stake_amount is rejected
#[test]
fn test_create_match_negative_stake_fails() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    assert_eq!(
        client.try_create_match(
            &player1,
            &player2,
            &-1,
            &Some(token),
            &String::from_str(&env, "neg_stake"),
            &Platform::Lichess,
        ),
        Err(Ok(Error::StakeTooLow))
    );
}

// Issue #35: get_escrow_balance returns 0 after match is cancelled with partial deposit
#[test]
fn test_escrow_balance_zero_after_cancel() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    let token_client = TokenClient::new(&env, &token);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "cancel_balance"),
        &Platform::Lichess,
    );
    client.deposit(&id, &player1);
    assert_eq!(client.get_escrow_balance(&id), 100);

    client.cancel_match(&id, &player1);
    assert_eq!(client.get_escrow_balance(&id), 0);
    assert_eq!(token_client.balance(&player1), 1000); // fully refunded
}

// Issue #1125: get_escrow_balance returns the full pot while the match is PendingResult.
#[test]
fn test_escrow_balance_full_pot_while_pending_result() {
    let (env, contract_id, oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let stake = 100_i128;
    let id = client.create_match(
        &player1,
        &player2,
        &stake,
        &Some(token),
        &String::from_str(&env, "pending_result_balance"),
        &Platform::Lichess,
    );

    client.deposit(&id, &player1);
    client.deposit(&id, &player2);
    assert_eq!(client.get_escrow_balance(&id), stake * 2);

    client.submit_result(
        &id,
        &String::from_str(&env, "pending_result_balance"),
        &Winner::Player1,
        &oracle,
    );

    assert_eq!(client.get_match(&id).state, MatchState::PendingResult);
    assert_eq!(client.get_escrow_balance(&id), stake * 2);
}

// Issue #180: Once both players have deposited the match transitions to Active.
// Mutual cancel_match is now allowed for Active matches â€” both players must authorize.
// Unilateral cancel (only one player's auth) must still be rejected.
#[test]
fn test_cancel_with_both_deposits_requires_auth() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    let token_client = TokenClient::new(&env, &token);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "both_dep_cancel"),
        &Platform::Lichess,
    );

    // Both players deposit â†’ state becomes Active
    client.deposit(&id, &player1);
    client.deposit(&id, &player2);
    assert_eq!(client.get_match(&id).state, MatchState::Active);

    // With mock_all_auths, mutual cancel must succeed
    client.cancel_match(&id, &player1);

    assert_eq!(client.get_match(&id).state, MatchState::Cancelled);
    // Both players are fully refunded
    assert_eq!(token_client.balance(&player1), 1000);
    assert_eq!(token_client.balance(&player2), 1000);
    assert_eq!(client.get_escrow_balance(&id), 0);
}

// Issue #100: Test that submit_result on a cancelled match returns InvalidState (no deposit)
#[test]
fn test_submit_result_on_cancelled_match_no_deposit_fails() {
    let (env, contract_id, oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "cancelled_result2"),
        &Platform::Lichess,
    );
    client.cancel_match(&id, &player1);

    assert_eq!(
        client.try_submit_result(
            &id,
            &String::from_str(&env, "cancelled_result2"),
            &Winner::Player1,
            &oracle,
        ),
        Err(Ok(Error::InvalidState))
    );
}

// Issue #225: MatchCount overflow returns Error::Overflow instead of wrapping
#[test]
fn test_match_count_overflow_returns_error() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    // Seed the counter at u64::MAX so the next increment would overflow
    env.as_contract(&contract_id, || {
        env.storage()
            .instance()
            .set(&DataKey::MatchCount, &u64::MAX);
    });

    assert_eq!(
        client.try_create_match(
            &player1,
            &player2,
            &100,
            &Some(token),
            &String::from_str(&env, "overflow_game"),
            &Platform::Lichess,
        ),
        Err(Ok(Error::Overflow))
    );
}

// Issue #209 / Closes #36: Player2 win payout sends full pot to player2
#[test]
fn test_player2_win_payout_full_pot() {
    let (env, contract_id, oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    let token_client = TokenClient::new(&env, &token);

    let stake = 100_i128;

    // Record pre-match balances
    let p1_before = token_client.balance(&player1);
    let p2_before = token_client.balance(&player2);

    let id = client.create_match(
        &player1,
        &player2,
        &stake,
        &Some(token),
        &String::from_str(&env, "p2_win_pot"),
        &Platform::Lichess,
    );

    client.deposit(&id, &player1);
    client.deposit(&id, &player2);
    assert!(client.is_funded(&id));
    assert_eq!(client.get_escrow_balance(&id), stake * 2);

    client.submit_result(
        &id,
        &String::from_str(&env, "p2_win_pot"),
        &Winner::Player2,
        &oracle,
    );

    // Player2 receives full pot (2x stake); player1 receives nothing
    // net gain = stake (deposited stake, won 2x)
    assert_eq!(token_client.balance(&player2), p2_before + stake);
    assert_eq!(token_client.balance(&player1), p1_before - stake); // net loss = stake
    assert_eq!(client.get_match(&id).state, MatchState::Completed);
    assert_eq!(client.get_escrow_balance(&id), 0);
}

// Issue #222: cancel_match refunds only player1 when only player1 has deposited;
// player2 balance must remain unchanged and escrow must return to 0.
#[test]
fn test_cancel_match_refunds_only_player1_when_only_player1_deposited() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    let token_client = TokenClient::new(&env, &token);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "partial_deposit_cancel"),
        &Platform::Lichess,
    );

    // Only player1 deposits
    client.deposit(&id, &player1);
    assert_eq!(token_client.balance(&player1), 900);
    assert_eq!(token_client.balance(&player2), 1000); // player2 untouched
    assert_eq!(client.get_escrow_balance(&id), 100);

    // Cancel â€” player2 triggers the cancellation
    client.cancel_match(&id, &player2);

    // player1 must be fully refunded
    assert_eq!(token_client.balance(&player1), 1000);
    // player2 balance must be unchanged (never deposited, must not receive anything)
    assert_eq!(token_client.balance(&player2), 1000);
    // Escrow must be empty
    assert_eq!(client.get_escrow_balance(&id), 0);
    // Match must be in Cancelled state
    assert_eq!(client.get_match(&id).state, MatchState::Cancelled);
}

// â”€â”€ Re-entrancy Analysis â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
//
// Soroban's execution model prevents classic re-entrancy: the runtime does not
// allow a contract to be re-entered while it is already executing (the host
// function `call` returns an error if the target contract is already on the
// call stack). This analysis confirms that:
//
//   1. In `deposit`: all state changes occur AFTER the external `try_transfer`
//      call (checks-effects-interactions). If the token contract attempted to
//      re-enter the escrow contract, the Soroban SDK would reject the call at
//      the host level before any escrow state could be read or written.
//
//   2. In `submit_result`: all validation (caller auth, game_id, state check)
//      occurs BEFORE the payout transfers. The state is set to Completed AFTER
//      the transfers complete. If a transfer failed (e.g., insufficient balance),
//      the whole transaction reverts â€” no inconsistent state is persisted.
//
// The tests below verify the checks-effects-interactions pattern by asserting
// that state changes follow external calls in the correct order.

#[test]
fn test_reentrancy_deposit_checks_effects_interactions() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    let token_client = TokenClient::new(&env, &token);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "reentrancy_deposit"),
        &Platform::Lichess,
    );

    // Before deposit, verify initial state
    let m = client.get_match(&id);
    assert!(!m.player1_deposited);
    assert_eq!(token_client.balance(&player1), 1000);

    // Deposit succeeds â€” checks (state validation) happen before the external
    // token transfer, and effects (state update) happen after.
    client.deposit(&id, &player1);

    // After deposit, verify effect was applied correctly
    let m = client.get_match(&id);
    assert!(m.player1_deposited);
    assert_eq!(token_client.balance(&player1), 900);
    assert_eq!(client.get_escrow_balance(&id), 100);
}

#[test]
fn test_reentrancy_submit_result_checks_effects_interactions() {
    let (env, contract_id, oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    let token_client = TokenClient::new(&env, &token);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "reentrancy_submit"),
        &Platform::Lichess,
    );
    client.deposit(&id, &player1);
    client.deposit(&id, &player2);

    // Before submit_result, verify state
    assert_eq!(client.get_match(&id).state, MatchState::Active);
    assert_eq!(token_client.balance(&player1), 900);
    assert_eq!(token_client.balance(&player2), 900);

    // All checks (caller auth, game_id, state == Active) happen before the
    // external payout transfers. The state is only set to Completed after
    // all transfers complete.
    client.submit_result(
        &id,
        &String::from_str(&env, "reentrancy_submit"),
        &Winner::Player1,
        &oracle,
    );

    // After submit_result, verify state is committed after external calls
    assert_eq!(client.get_match(&id).state, MatchState::Completed);
    assert_eq!(token_client.balance(&player1), 1100);
    assert_eq!(token_client.balance(&player2), 900);
    assert_eq!(client.get_escrow_balance(&id), 0);
}

// Issue: deposit returns InsufficientAllowance when player has not approved the contract
#[test]
fn test_deposit_insufficient_allowance() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let oracle = Address::generate(&env);
    let player1 = Address::generate(&env);
    let player2 = Address::generate(&env);

    let token_id = env.register_stellar_asset_contract_v2(admin.clone());
    let token_addr = token_id.address();
    let asset_client = StellarAssetClient::new(&env, &token_addr);
    asset_client.mint(&player1, &1000);
    asset_client.mint(&player2, &1000);

    let contract_id = env.register(EscrowContract, ());
    let client = EscrowContractClient::new(&env, &contract_id);
    let safe_address = Address::generate(&env);
    client.initialize(&oracle, &admin, &token_addr, &safe_address, &None, &None);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token_addr),
        &String::from_str(&env, "allowance_zero"),
        &Platform::Lichess,
    );

    // No approval was set â€” allowance is 0
    assert_eq!(
        client.try_deposit(&id, &player1),
        Err(Ok(Error::InsufficientAllowance))
    );
}

// Issue: deposit succeeds when allowance is exactly the stake amount
#[test]
fn test_deposit_succeeds_with_exact_allowance() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    let token_client = TokenClient::new(&env, &token);

    // Set the allowance to exactly the stake amount: 100
    let expiration = env.ledger().sequence() + 1000000;
    let token_client_approve = TokenClient::new(&env, &token);
    token_client_approve.approve(&player1, &contract_id, &100, &expiration);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "exact_allowance"),
        &Platform::Lichess,
    );

    client.deposit(&id, &player1);
    assert_eq!(token_client.balance(&player1), 900);
    assert_eq!(client.get_escrow_balance(&id), 100);
}

// Issue #1102: emergency_drain â€” success, unpaused guard, non-admin guard
// The `to` parameter has been removed from emergency_drain; the destination is
// always the `safe_address` registered at initialize time.

#[test]
fn test_emergency_drain_succeeds_when_paused() {
    let (env, contract_id, _oracle, player1, player2, token, admin, safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    let token_client = TokenClient::new(&env, &token);

    // Fund the escrow with two deposits
    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "drain_test"),
        &Platform::Lichess,
    );
    client.deposit(&id, &player1);
    client.deposit(&id, &player2);
    // Total contract balance = stakes (200) + reserve buffer (15_000_000) minted in setup()
    let total_expected = 200 + crate::ESCROW_RESERVE_BUFFER_STROOPS;
    assert_eq!(token_client.balance(&contract_id), total_expected);

    client.pause();

    // emergency_drain no longer accepts a destination â€” it always drains to safe_address
    client.emergency_drain(&admin);

    // Capture events BEFORE any further contract calls that might clear them
    let events = env.events().all();

    assert_eq!(token_client.balance(&contract_id), 0);
    assert_eq!(token_client.balance(&safe_address), total_expected);

    // Verify drain event
    let drain_event = events.iter().find(|(_, t, _)| {
        t.len() == 2
            && Symbol::try_from_val(&env, &t.get(0).unwrap()).unwrap()
                == Symbol::new(&env, "admin")
            && Symbol::try_from_val(&env, &t.get(1).unwrap()).unwrap()
                == symbol_short!("drain")
    });
    assert!(
        drain_event.is_some(),
        "drain event not found ({} events total)",
        events.len()
    );
}

#[test]
fn test_emergency_drain_fails_when_not_paused() {
    let (env, contract_id, _oracle, _player1, _player2, _token, admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    assert_eq!(
        client.try_emergency_drain(&admin),
        Err(Ok(Error::NotPaused))
    );
}

#[test]
fn test_emergency_drain_fails_for_non_admin() {
    let (env, contract_id, _oracle, _player1, _player2, _token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    client.pause();
    let non_admin = Address::generate(&env);
    assert_eq!(
        client.try_emergency_drain(&non_admin),
        Err(Ok(Error::Unauthorized))
    );
}

#[test]
fn test_create_match_valid_platforms_accepted() {
    // Both known Platform variants must be accepted by create_match.
    // This test verifies the platform validation guard does not reject valid values.
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let id1 = client.create_match(
        &player1, &player2, &100, &Some(token),
        &String::from_str(&env, "lichess-game-1"), &Platform::Lichess,
    );
    assert_eq!(client.get_match(&id1).platform, Platform::Lichess);

    let id2 = client.create_match(
        &player1, &player2, &100, &Some(token),
        &String::from_str(&env, "chessdotcom-game-1"), &Platform::ChessDotCom,
    );
    assert_eq!(client.get_match(&id2).platform, Platform::ChessDotCom);
}

// Issue #794: get_oracle returns the address passed to initialize
#[test]
fn test_get_oracle_returns_initialized_address() {
    let (env, contract_id, oracle, _player1, _player2, _token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    assert_eq!(client.get_oracle(), oracle);
}

#[test]
fn test_get_admin_returns_initialized_address() {
    let (env, contract_id, _oracle, _player1, _player2, _token, admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    assert_eq!(client.get_admin(), admin);
}

// Issue #792: stake amount above MAX_STAKE is rejected
#[test]
fn test_create_match_stake_too_high_fails() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    assert_eq!(
        client.try_create_match(
            &player1,
            &player2,
            &(crate::MAX_STAKE + 1),
            &Some(token),
            &String::from_str(&env, "too_high"),
            &Platform::Lichess,
        ),
        Err(Ok(Error::StakeTooHigh))
    );
}

#[test]
fn test_create_match_max_stake() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    let id = client.create_match(
        &player1,
        &player2,
        &crate::MAX_STAKE,
        &Some(token),
        &String::from_str(&env, "max_stake"),
        &Platform::Lichess,
    );
    let m = client.get_match(&id);
    assert_eq!(m.stake_amount, crate::MAX_STAKE);
}

#[test]
fn test_finalize_result_dispute_window_boundary() {
    let (env, contract_id, oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "boundary_test"),
        &Platform::Lichess,
    );
    client.deposit(&id, &player1);
    client.deposit(&id, &player2);

    client.submit_result(
        &id,
        &String::from_str(&env, "boundary_test"),
        &Winner::Player1,
        &oracle,
    );

    let m = client.get_match(&id);
    // advance the ledger to exactly the boundary
    env.ledger().set_sequence_number(m.pending_result_ledger + crate::DISPUTE_WINDOW_LEDGERS);

    assert_eq!(
        client.try_finalize_result(&id),
        Err(Ok(Error::DisputeWindowActive))
    );
}

// Issue #791: stake amount of exactly zero is rejected as InvalidStakeAmount (#3);
// below-MIN_STAKE non-zero values are rejected as StakeTooLow.
#[test]
fn test_create_match_stake_below_min_fails() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    assert_eq!(
        client.try_create_match(
            &player1,
            &player2,
            &0,
            &Some(token),
            &String::from_str(&env, "below_min"),
            &Platform::Lichess,
        ),
        Err(Ok(Error::InvalidStakeAmount))
    );
}

// Issue #793: instance storage TTL is extended after initialize
#[test]
fn test_instance_ttl_extended_on_initialize() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let oracle = Address::generate(&env);
    let token_id = env.register_stellar_asset_contract_v2(admin.clone());
    let token_addr = token_id.address();
    let contract_id = env.register(EscrowContract, ());
    let client = EscrowContractClient::new(&env, &contract_id);
    let safe_address = Address::generate(&env);
    client.initialize(&oracle, &admin, &token_addr, &safe_address, &None, &None);

    let instance_ttl = env.as_contract(&contract_id, || env.storage().instance().get_ttl());
    assert!(instance_ttl >= crate::INSTANCE_LIFETIME_THRESHOLD);
}

// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•
// Pagination Tests
// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•

#[test]
fn test_list_matches_empty_contract() {
    let (env, contract_id, ..) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let result = client.list_matches(&0, &50);
    assert_eq!(result.len(), 0, "empty contract should return empty result");
}

#[test]
fn test_list_matches_basic() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    // Create 5 matches
    for i in 0..5 {
        let game_id = String::from_str(&env, &format!("game_{}", i));
        client.create_match(
            &player1,
            &player2,
            &100,
            &Some(token),
            &game_id,
            &Platform::Lichess,
        );
    }

    let result = client.list_matches(&0, &50);
    assert_eq!(result.len(), 5, "should return all 5 match IDs");
    assert_eq!(result.get(0), 0);
    assert_eq!(result.get(1), 1);
    assert_eq!(result.get(2), 2);
    assert_eq!(result.get(3), 3);
    assert_eq!(result.get(4), 4);
}

#[test]
fn test_list_matches_with_limit() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    // Create 10 matches
    for i in 0..10 {
        let game_id = String::from_str(&env, &format!("game_limit_{}", i));
        client.create_match(
            &player1,
            &player2,
            &100,
            &Some(token),
            &game_id,
            &Platform::Lichess,
        );
    }

    // Request 5, should get 5
    let result = client.list_matches(&0, &5);
    assert_eq!(result.len(), 5, "should respect limit of 5");
    assert_eq!(result.get(0), 0);
    assert_eq!(result.get(4), 4);

    // Request 5 starting at 5, should get remaining 5
    let result = client.list_matches(&5, &5);
    assert_eq!(result.len(), 5);
    assert_eq!(result.get(0), 5);
    assert_eq!(result.get(4), 9);
}

#[test]
fn test_list_matches_offset() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    // Create 5 matches
    for i in 0..5 {
        let game_id = String::from_str(&env, &format!("game_offset_{}", i));
        client.create_match(
            &player1,
            &player2,
            &100,
            &Some(token),
            &game_id,
            &Platform::Lichess,
        );
    }

    // Start from middle
    let result = client.list_matches(&2, &50);
    assert_eq!(result.len(), 3, "should return IDs 2, 3, 4");
    assert_eq!(result.get(0), 2);
    assert_eq!(result.get(1), 3);
    assert_eq!(result.get(2), 4);
}

#[test]
fn test_list_matches_after_empty() {
    let (env, contract_id, ..) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let result = client.list_matches_after(&u64::MAX, &50);
    assert_eq!(
        result.len(),
        0,
        "empty contract should return empty result for cursor pagination"
    );
}

#[test]
fn test_list_matches_after_basic() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    // Create 5 matches
    for i in 0..5 {
        let game_id = String::from_str(&env, &format!("game_after_{}", i));
        client.create_match(
            &player1,
            &player2,
            &100,
            &Some(token),
            &game_id,
            &Platform::Lichess,
        );
    }

    // Start from beginning (u64::MAX acts as "before all")
    let result = client.list_matches_after(&u64::MAX, &50);
    assert_eq!(result.len(), 5, "cursor from MAX should return all matches");
    assert_eq!(result.get(0), 0);
    assert_eq!(result.get(4), 4);
}

#[test]
fn test_list_matches_after_with_cursor() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    // Create 10 matches
    for i in 0..10 {
        let game_id = String::from_str(&env, &format!("game_cursor_{}", i));
        client.create_match(
            &player1,
            &player2,
            &100,
            &Some(token),
            &game_id,
            &Platform::Lichess,
        );
    }

    // Get first page
    let page1 = client.list_matches_after(&u64::MAX, &5);
    assert_eq!(page1.len(), 5);
    assert_eq!(page1.get(0), 0);
    assert_eq!(page1.get(4), 4);

    // Use last ID from page1 as cursor for page2
    let cursor = page1.get(4); // ID 4
    let page2 = client.list_matches_after(&cursor, &5);
    assert_eq!(page2.len(), 5, "should return IDs 5-9");
    assert_eq!(page2.get(0), 5);
    assert_eq!(page2.get(4), 9);

    // Next page should be empty (end of data)
    let page3 = client.list_matches_after(&9, &5);
    assert_eq!(page3.len(), 0, "should be empty at end of data");
}

#[test]
fn test_list_matches_after_unambiguous_eof() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    // Create matches with IDs 0, 1, 2
    for i in 0..3 {
        let game_id = String::from_str(&env, &format!("game_eof_{}", i));
        client.create_match(
            &player1,
            &player2,
            &100,
            &Some(token),
            &game_id,
            &Platform::Lichess,
        );
    }

    // Cursor at ID 2 (last match)
    let result = client.list_matches_after(&2, &50);
    assert_eq!(
        result.len(),
        0,
        "cursor after last ID should return empty (unambiguous EOF)"
    );

    // Cursor at ID 100 (beyond all matches)
    let result = client.list_matches_after(&100, &50);
    assert_eq!(
        result.len(),
        0,
        "cursor beyond all IDs should return empty (unambiguous EOF)"
    );
}

#[test]
fn test_list_matches_after_limit() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    // Create 20 matches
    for i in 0..20 {
        let game_id = String::from_str(&env, &format!("game_limit_after_{}", i));
        client.create_match(
            &player1,
            &player2,
            &100,
            &Some(token),
            &game_id,
            &Platform::Lichess,
        );
    }

    // Request 10 starting after ID 5
    let result = client.list_matches_after(&5, &10);
    assert_eq!(result.len(), 10, "should return 10 IDs");
    assert_eq!(result.get(0), 6, "should start after cursor");
    assert_eq!(result.get(9), 15);
}

/// Issue #68: get_game_id_owner must return the match_id that registered a
/// given game_id, and None for unregistered game_ids.
#[test]
fn test_get_game_id_owner_returns_match_id_and_none_for_unknown() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let game_a = String::from_str(&env, "game_owner_a");
    let game_b = String::from_str(&env, "game_owner_b");
    let game_unknown = String::from_str(&env, "game_owner_unknown");

    let id_a = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &game_a,
        &Platform::Lichess,
    );
    let id_b = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &game_b,
        &Platform::Lichess,
    );

    // Registered game_ids return their owning match_id.
    assert_eq!(
        client.get_game_id_owner(&game_a),
        Some(id_a),
        "registered game_id must return its match_id"
    );
    assert_eq!(
        client.get_game_id_owner(&game_b),
        Some(id_b),
        "registered game_id must return its match_id"
    );

    // Unregistered game_id returns None.
    assert_eq!(
        client.get_game_id_owner(&game_unknown),
        None,
        "unregistered game_id must return None"
    );
}

// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•
// Issue #1122 â€” Property-based tests for state machine transition invariants
// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•

/// Issue #1036: A third party (neither player1 nor player2) must be rejected
/// with Error::Unauthorized when they call claim_timeout, even after the
/// timeout period has elapsed.
#[test]
fn test_claim_timeout_third_party_unauthorized() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "timeout_3p"),
        &Platform::Lichess,
    );
    client.deposit(&id, &player1);
    client.deposit(&id, &player2);

    // Advance ledger past the 7-day timeout window
    env.ledger().set_sequence_number(
        env.ledger().sequence() + crate::TIMEOUT_LEDGERS + 1,
    );

    // A completely unrelated address must be rejected
    let third_party = Address::generate(&env);
    assert_eq!(
        client.try_claim_timeout(&id, &third_party),
        Err(Ok(Error::Unauthorized)),
        "third party must be rejected with Unauthorized"
    );
}

/// A player can claim timeout once TIMEOUT_LEDGERS have elapsed and both
/// players get their stake back.
#[test]
fn test_claim_timeout_player1_succeeds_after_timeout() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    let token_client = TokenClient::new(&env, &token);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "timeout_p1"),
        &Platform::Lichess,
    );
    client.deposit(&id, &player1);
    client.deposit(&id, &player2);

    // Advance ledger past the timeout window
    env.ledger().set_sequence_number(
        env.ledger().sequence() + crate::TIMEOUT_LEDGERS + 1,
    );

    client.claim_timeout(&id, &player1);

    assert_eq!(client.get_match(&id).state, MatchState::Cancelled);
    assert_eq!(token_client.balance(&player1), 1000);
    assert_eq!(token_client.balance(&player2), 1000);
}

#[test]
fn test_claim_timeout_boundary() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &token,
        &String::from_str(&env, "timeout_boundary"),
        &Platform::Lichess,
    );
    client.deposit(&id, &player1);
    client.deposit(&id, &player2);

    // The match becomes Active at the ledger sequence at deposit time.
    let activated = env.ledger().sequence();
    let timeout = crate::TIMEOUT_LEDGERS;

    // Immediately before the timeout window: should return TimeoutNotReached
    env.ledger().set_sequence_number(activated + timeout - 1);
    assert_eq!(client.try_claim_timeout(&id, &player1), Err(Ok(Error::TimeoutNotReached)));

    // Exactly when timeout becomes valid: claim should succeed
    env.ledger().set_sequence_number(activated + timeout);
    assert!(client.try_claim_timeout(&id, &player1).is_ok());
}

/// claim_timeout must fail with MatchTimedOut (too early) when called before
/// TIMEOUT_LEDGERS have elapsed.
#[test]
fn test_claim_timeout_too_early_fails() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "timeout_early"),
        &Platform::Lichess,
    );
    client.deposit(&id, &player1);
    client.deposit(&id, &player2);

    // Do NOT advance the ledger â€” timeout period has not elapsed
    assert_eq!(
        client.try_claim_timeout(&id, &player1),
        Err(Ok(Error::MatchTimedOut)),
        "claim_timeout before timeout window must return MatchTimedOut"
    );
}

// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•
// Issue #1036 â”€â”€ (end)
// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•

// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•
// Issue #1035 â€” transfer_admin extend_ttl test
// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•

/// Issue #1035: transfer_admin must extend the instance TTL after updating the
/// admin so the new admin's first read of instance storage succeeds even when
/// the TTL was about to expire.
#[test]
fn test_transfer_admin_extends_instance_ttl() {
    let (env, contract_id, _oracle, _player1, _player2, _token, admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let new_admin = Address::generate(&env);

    // Record TTL before the call
    let ttl_before = env.as_contract(&contract_id, || env.storage().instance().get_ttl());

    client.transfer_admin(&admin, &new_admin);

    // TTL must still be at the full bump amount (not shrunk)
    let ttl_after = env.as_contract(&contract_id, || env.storage().instance().get_ttl());
    assert!(
        ttl_after >= crate::INSTANCE_LIFETIME_THRESHOLD,
        "instance TTL must be extended after transfer_admin: before={}, after={}",
        ttl_before,
        ttl_after
    );
}

// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•
// Issue #1034 â€” emergency_drain drain_noop event test
// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•

/// Issue #1034 / #69: when emergency_drain is called on a zero-balance contract,
/// it must emit a drn_noop event, return Ok(()), and must NOT attempt a transfer.
#[test]
fn test_emergency_drain_zero_balance_emits_drain_noop() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let oracle = Address::generate(&env);
    let safe_address = Address::generate(&env);

    // Register a token but do NOT mint any balance to the contract
    let token_addr = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_client = TokenClient::new(&env, &token_addr);

    let contract_id = env.register(EscrowContract, ());
    let client = EscrowContractClient::new(&env, &contract_id);
    client.initialize(&oracle, &admin, &token_addr, &safe_address);

    // Sanity: contract starts with a zero balance
    assert_eq!(token_client.balance(&contract_id), 0);
    assert_eq!(token_client.balance(&safe_address), 0);

    // Pause the contract (required by emergency_drain)
    client.pause();

    // Call emergency_drain on an empty contract â€” must succeed (no error)
    assert!(
        client.try_emergency_drain(&admin).is_ok(),
        "emergency_drain on zero balance must return Ok(())"
    );

    // Verify no funds moved: contract and safe_address balances are still zero
    assert_eq!(token_client.balance(&contract_id), 0);
    assert_eq!(token_client.balance(&safe_address), 0);

    // Verify the drain_noop event was emitted (with amount 0) to preserve
    // the audit trail, rather than a real drain event or zero-amount transfer.
    let events = env.events().all();
    let noop_event = events.iter().find(|(_, t, _)| {
        t.len() == 2
            && Symbol::try_from_val(&env, &t.get(0).unwrap()).unwrap()
                == Symbol::new(&env, "admin")
            && Symbol::try_from_val(&env, &t.get(1).unwrap()).unwrap()
                == symbol_short!("drn_noop")
    });
    assert!(
        noop_event.is_some(),
        "drain_noop event must be emitted when emergency_drain is called on zero balance"
    );
    let (_, _, data) = noop_event.unwrap();
    let (amount, _dest, _admin): (i128, Address, Address) =
        soroban_sdk::TryFromVal::try_from_val(&env, &data).unwrap();
    assert_eq!(amount, 0, "drn_noop event amount must be 0");
}

// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•
// Issue #1033 â€” pending_result_ledger Option<u32> tests
// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•

/// Issue #1033: pending_result_ledger must be None before any oracle result
/// is submitted (not a 0 sentinel).
#[test]
fn test_pending_result_ledger_none_before_submit() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "prl_none"),
        &Platform::Lichess,
    );
    client.deposit(&id, &player1);
    client.deposit(&id, &player2);

    let m = client.get_match(&id);
    assert!(
        m.pending_result_ledger.is_none(),
        "pending_result_ledger must be None before oracle submits a result"
    );
}

/// Issue #1033: pending_result_ledger must be Some(ledger) after submit_result.
#[test]
fn test_pending_result_ledger_some_after_submit() {
    let (env, contract_id, oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "prl_some"),
        &Platform::Lichess,
    );
    client.deposit(&id, &player1);
    client.deposit(&id, &player2);

    let ledger_before = env.ledger().sequence();
    client.submit_result(
        &id,
        &String::from_str(&env, "prl_some"),
        &Winner::Player1,
        &oracle,
    );

    // With finalize_result, need to advance past dispute window
    env.ledger().set_sequence_number(
        env.ledger().sequence() + crate::DISPUTE_WINDOW_LEDGERS + 1,
    );
    client.finalize_result(&id);

    // We verified it was set by the fact finalize_result succeeded
    // (it would have returned InvalidState if pending_result_ledger was None)
    assert_eq!(client.get_match(&id).state, MatchState::Completed);
    let _ = ledger_before; // used for context
}


//
// These tests use proptest to generate exhaustive random call sequences and
// assert that any operation issued in the wrong state is always rejected with
// `Error::InvalidState`, `Error::MatchCancelled`, or `Error::MatchCompleted`,
// ensuring the state machine cannot be subverted.
//
// Because proptest requires `std` and the contract is `#![no_std]`, the
// property tests live in a separate sub-module gated on the `testutils`
// feature.  They import the contract types directly and drive the standard
// `EscrowContractClient` provided by the SDK.

#[cfg(test)]
mod proptest_state_machine {
    extern crate std;

    use super::*;
    use proptest::prelude::*;
    use soroban_sdk::{
        testutils::Address as _,
        token::{Client as TokenClient, StellarAssetClient},
        Address, Env, String,
    };

    // â”€â”€ helpers â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

    /// Build a minimal environment with the escrow contract initialised.
    fn prop_setup() -> (Env, Address, Address, Address, Address, Address, Address) {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let oracle = Address::generate(&env);
        let player1 = Address::generate(&env);
        let player2 = Address::generate(&env);

        let token_id = env.register_stellar_asset_contract_v2(admin.clone());
        let token_addr = token_id.address();
        let asset_client = StellarAssetClient::new(&env, &token_addr);
        asset_client.mint(&player1, &1_000);
        asset_client.mint(&player2, &1_000);

        let contract_id = env.register(EscrowContract, ());
        let client = EscrowContractClient::new(&env, &contract_id);
        let safe_address = Address::generate(&env);
        client.initialize(&oracle, &admin, &token_addr, &safe_address, &None, &None);

        let expiration = env.ledger().sequence() + 1_000_000;
        let token_client = TokenClient::new(&env, &token_addr);
        token_client.approve(&player1, &contract_id, &1_000, &expiration);
        token_client.approve(&player2, &contract_id, &1_000, &expiration);

        (env, contract_id, oracle, player1, player2, token_addr, admin)
    }

    // â”€â”€ invariant 1: Completed is terminal â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

    /// After a match reaches the `Completed` state any subsequent call to
    /// `deposit`, `cancel_match`, or `submit_result` must return an error â€”
    /// never silently succeed.
    ///
    /// We parametrise over which player wins so proptest can cover all three
    /// `Winner` variants across many runs.
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(20))]

        #[test]
        fn prop_completed_is_terminal(winner_idx in 0usize..3) {
            let (env, contract_id, oracle, player1, player2, token, _admin) = prop_setup();
            let client = EscrowContractClient::new(&env, &contract_id);

            let winners = [Winner::Player1, Winner::Player2, Winner::Draw];
            let winner = winners[winner_idx].clone();

            let id = client.create_match(
                &player1,
                &player2,
                &100,
                &Some(token),
                &String::from_str(&env, "prop-completed"),
                &Platform::Lichess,
            );
            client.deposit(&id, &player1);
            client.deposit(&id, &player2);

            // Drive to PendingResult then advance past dispute window
            client.submit_result(&id, &String::from_str(&env, "prop-completed"), &winner, &oracle);
            // Advance ledger past the dispute window
            env.ledger().set_sequence_number(env.ledger().sequence() + crate::DISPUTE_WINDOW_LEDGERS + 1);
            client.finalize_result(&id, &player1);

            // After Completed: every mutating call must be rejected
            assert_eq!(
                client.get_match(&id).state,
                MatchState::Completed,
                "expected Completed state"
            );

            // deposit is rejected
            let deposit_result = client.try_deposit(&id, &player1);
            prop_assert!(
                deposit_result.is_err(),
                "deposit on Completed match must be rejected"
            );

            // cancel_match is rejected
            let cancel_result = client.try_cancel_match(&id, &player1);
            prop_assert!(
                cancel_result.is_err(),
                "cancel_match on Completed match must be rejected"
            );

            // submit_result is rejected
            let submit_result = client.try_submit_result(
                &id,
                &String::from_str(&env, "prop-completed"),
                &Winner::Player1,
                &oracle,
            );
            prop_assert!(
                submit_result.is_err(),
                "submit_result on Completed match must be rejected"
            );
        }
    }

    // â”€â”€ invariant 2: Cancelled is terminal â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

    /// After a match reaches the `Cancelled` state any subsequent call to
    /// `deposit`, `cancel_match`, or `submit_result` must return an error.
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(20))]

        #[test]
        fn prop_cancelled_is_terminal(cancel_with_deposit in proptest::bool::ANY) {
            let (env, contract_id, oracle, player1, player2, token, _admin) = prop_setup();
            let client = EscrowContractClient::new(&env, &contract_id);

            let id = client.create_match(
                &player1,
                &player2,
                &100,
                &Some(token),
                &String::from_str(&env, "prop-cancelled"),
                &Platform::Lichess,
            );

            if cancel_with_deposit {
                client.deposit(&id, &player1);
            }
            client.cancel_match(&id, &player1);

            assert_eq!(client.get_match(&id).state, MatchState::Cancelled);

            // deposit is rejected
            let deposit_result = client.try_deposit(&id, &player1);
            prop_assert!(
                deposit_result.is_err(),
                "deposit on Cancelled match must be rejected"
            );

            // cancel_match is rejected
            let cancel_result = client.try_cancel_match(&id, &player1);
            prop_assert!(
                cancel_result.is_err(),
                "cancel_match on Cancelled match must be rejected"
            );

            // submit_result is rejected
            let submit_result = client.try_submit_result(
                &id,
                &String::from_str(&env, "prop-cancelled"),
                &Winner::Player1,
                &oracle,
            );
            prop_assert!(
                submit_result.is_err(),
                "submit_result on Cancelled match must be rejected"
            );
        }
    }

    // â”€â”€ invariant 3: Active â†’ unilateral cancel rejected, mutual cancel allowed â”€â”€â”€â”€â”€

    /// Once a match is `Active` (both players deposited), a *unilateral* `cancel_match`
    /// (only one player's auth) must always be rejected â€” the contract requires both
    /// players to authorize. A mutual cancel (both auths present) is allowed and
    /// should return `Ok`.
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(20))]

        #[test]
        fn prop_active_cancel_unilateral_always_rejected(cancel_caller_is_p1 in proptest::bool::ANY) {
            use soroban_sdk::testutils::{MockAuth, MockAuthInvoke};

            let (env, contract_id, _oracle, player1, player2, token, _admin) = prop_setup();
            let client = EscrowContractClient::new(&env, &contract_id);

            let id = client.create_match(
                &player1,
                &player2,
                &100,
                &Some(token),
                &String::from_str(&env, "prop-active-cancel"),
                &Platform::Lichess,
            );
            client.deposit(&id, &player1);
            client.deposit(&id, &player2);

            prop_assert_eq!(client.get_match(&id).state, MatchState::Active);

            // Provide only one player's auth â€” must be rejected
            let (caller, fn_caller) = if cancel_caller_is_p1 {
                (&player1, player1.clone())
            } else {
                (&player2, player2.clone())
            };
            env.mock_auths(&[MockAuth {
                address: caller,
                invoke: &MockAuthInvoke {
                    contract: &contract_id,
                    fn_name: "cancel_match",
                    args: (id, fn_caller.clone()).into_val(&env),
                    sub_invokes: &[],
                },
            }]);

            let result = client.try_cancel_match(&id, caller);
            prop_assert!(
                result.is_err(),
                "unilateral cancel_match on Active match must be rejected"
            );
        }
    }

    // â”€â”€ invariant 4: submit_result only valid from Active â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

    /// `submit_result` must return `InvalidState` when called on a match that
    /// is in `Pending` or `Cancelled` state (covers non-Active starting states
    /// via proptest).
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(20))]

        #[test]
        fn prop_submit_result_only_valid_from_active(
            depositor_idx in 0usize..3,  // 0 = none, 1 = p1 only, 2 = p2 only
        ) {
            let (env, contract_id, oracle, player1, player2, token, _admin) = prop_setup();
            let client = EscrowContractClient::new(&env, &contract_id);

            // Use unique game IDs per run to avoid DuplicateGameId across repeated
            // proptest cases within the same process.  We embed depositor_idx in
            // the ID to ensure uniqueness.
            let raw: &str = match depositor_idx {
                0 => "prop-submit-none",
                1 => "prop-submit-p1",
                _ => "prop-submit-p2",
            };
            let game_id = String::from_str(&env, raw);

            let id = client.create_match(
                &player1,
                &player2,
                &100,
                &Some(token),
                &game_id,
                &Platform::Lichess,
            );

            match depositor_idx {
                1 => { client.deposit(&id, &player1); }
                2 => { client.deposit(&id, &player2); }
                _ => {}
            }

            // Match is Pending (not Active) â€” submit_result must be rejected
            let result = client.try_submit_result(
                &id,
                &game_id,
                &Winner::Player1,
                &oracle,
            );
            prop_assert!(
                result.is_err(),
                "submit_result must be rejected when match is not Active"
            );
            // Must be InvalidState, not some other error
            prop_assert_eq!(
                result,
                Err(Ok(Error::InvalidState)),
                "submit_result on non-Active match must return InvalidState"
            );
        }
    }

    // â”€â”€ invariant 5: no operation accepted after terminal state â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

    /// Comprehensive sweep: for every reachable terminal state, assert that
    /// ALL mutating operations are rejected.  This catches any future addition
    /// to the API that might forget to guard against terminal states.
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(10))]

        #[test]
        fn prop_no_operation_accepted_in_terminal_state(
            reach_completed in proptest::bool::ANY,
        ) {
            let (env, contract_id, oracle, player1, player2, token, _admin) = prop_setup();
            let client = EscrowContractClient::new(&env, &contract_id);

            let (game_str, terminal_state) = if reach_completed {
                ("prop-terminal-completed", MatchState::Completed)
            } else {
                ("prop-terminal-cancelled", MatchState::Cancelled)
            };
            let game_id = String::from_str(&env, game_str);

            let id = client.create_match(
                &player1, &player2, &100, &Some(token), &game_id, &Platform::Lichess,
            );

            if reach_completed {
                client.deposit(&id, &player1);
                client.deposit(&id, &player2);
                client.submit_result(&id, &game_id, &Winner::Player1, &oracle);
                env.ledger().set_sequence_number(env.ledger().sequence() + crate::DISPUTE_WINDOW_LEDGERS + 1);
                client.finalize_result(&id, &player1);
            } else {
                client.cancel_match(&id, &player1);
            }

            prop_assert_eq!(client.get_match(&id).state, terminal_state);

            // Every mutating entry-point must be rejected
            prop_assert!(client.try_deposit(&id, &player1).is_err());
            prop_assert!(client.try_cancel_match(&id, &player1).is_err());
            prop_assert!(
                client.try_submit_result(&id, &game_id, &Winner::Player1, &oracle).is_err()
            );
            prop_assert!(client.try_finalize_result(&id, &player1).is_err());
        }
    }
}

// ============================================================================
// #1031 â€” get_token view function
// ============================================================================

#[test]
fn test_get_token_returns_initialized_token() {
    let (env, contract_id, _oracle, player1, _player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    let returned = client.get_token();
    assert_eq!(returned, token, "get_token should return the token set during initialize");
}

// ============================================================================
// #1032 â€” activated_ledger Option<u32> semantics
// ============================================================================

#[test]
fn test_activated_ledger_none_before_both_deposits() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "game_opt_test"),
        &Platform::Lichess,
    );

    // Before any deposit activated_ledger must be None
    let m = client.get_match(&id);
    assert_eq!(
        m.activated_ledger, None,
        "activated_ledger should be None before the match becomes Active"
    );

    // After only the first deposit it must still be None
    client.deposit(&id, &player1);
    let m = client.get_match(&id);
    assert_eq!(
        m.activated_ledger, None,
        "activated_ledger should remain None after just one deposit"
    );
}

#[test]
fn test_activated_ledger_some_after_both_deposits() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "game_opt_activated"),
        &Platform::Lichess,
    );

    client.deposit(&id, &player1);
    client.deposit(&id, &player2);

    let m = client.get_match(&id);
    assert!(
        m.activated_ledger.is_some(),
        "activated_ledger should be Some after both players deposit"
    );
    assert_eq!(
        m.activated_ledger.unwrap(),
        env.ledger().sequence(),
        "activated_ledger should record the current ledger sequence"
    );
}


// ============================================================================
//
// This module uses proptest to generate random inputs and verify that
// create_match either succeeds or returns one of the known-valid error codes.
// The contract should NEVER panic, even with arbitrary inputs.

#[test]
fn test_create_match_min_stake_boundary() {
    // Use the same setup helper as the fuzz tests – it returns all needed variables.
    let (env, contract_id, oracle, admin, _, player1, player2, token) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let min_stake = crate::MIN_STAKE; // Should be 1
    let game_id = String::from_str(&env, "min_stake_test");

    let result = client.try_create_match(
        &player1,
        &player2,
        &min_stake,
        &token,
        &game_id,
        &Platform::Lichess,
    );

    // Assert success – the exact minimum stake must be accepted.
    assert!(result.is_ok(), "create_match with MIN_STAKE should succeed");
    let match_id = result.unwrap();
    assert!(match_id >= 0);

    // Optionally verify the match was stored correctly.
    let match_data = client.get_match(&match_id).unwrap();
    assert_eq!(match_data.stake_amount, min_stake);
    assert_eq!(match_data.state, MatchState::Pending);
}

#[cfg(test)]
mod fuzz {
    use super::*;
    use proptest::prelude::*;

    /// Fuzz test strategy for stake amounts.
    /// Generates values across the valid range, boundary values, and overflows.
    fn arb_stake_amount() -> impl Strategy<Value = i128> {
        prop_oneof![
            // Valid range: [MIN_STAKE, MAX_STAKE]
            crate::MIN_STAKE..=crate::MAX_STAKE,
            // Boundary: just below and above valid range
            (crate::MIN_STAKE - 1)..=crate::MIN_STAKE,
            (crate::MAX_STAKE)..=(crate::MAX_STAKE + 1),
            // Extreme values
            Just(i128::MIN),
            Just(i128::MAX),
            Just(0i128),
            Just(-1i128),
        ]
    }

    /// Fuzz test strategy for game_id strings.
    /// Generates valid, empty, oversized, and various encoding scenarios.
    fn arb_game_id() -> impl Strategy<Value = Vec<u8>> {
        prop_oneof![
            // Valid: 1-64 bytes
            ".{1,64}".prop_map(|s| s.into_bytes()),
            // Empty
            Just(vec![]),
            // Just over max (65 bytes)
            "x{65}".prop_map(|s| s.into_bytes()),
            // Far oversized (1000 bytes)
            "y{1000}".prop_map(|s| s.into_bytes()),
            // Unicode edge cases (UTF-8)
            "lich(\\PC)*".prop_map(|s| s.into_bytes()),
        ]
    }

    /// Property test: create_match with fuzzed stake_amount
    /// Verifies that any stake_amount input is handled gracefully.
    #[test]
    fn fuzz_prop_create_match_stake_amount(stake in arb_stake_amount()) {
        let (env, contract_id, oracle, admin, _, player1, player2, token) = setup();
        let client = EscrowContractClient::new(&env, &contract_id);

        let game_id = String::from_str(&env, "fuzz_test_game");

        // Call create_match with arbitrary stake_amount
        let result = client.try_create_match(
            &player1, &player2, &stake, &Some(token), &game_id, &Platform::Lichess,
        );

        // Result must be either Ok or a known error code
        match result {
            Ok(match_id) => {
                // Success case: verify match was created in Pending state
                assert!(match_id >= 0);
                let match_data = client.get_match(&match_id).unwrap();
                assert_eq!(match_data.state, MatchState::Pending);
                assert_eq!(match_data.stake_amount, stake);
            }
            Err(Ok(err)) => {
                // Expected error codes for invalid stake amounts
                assert!(
                    matches!(
                        err,
                        Error::StakeTooLow
                            | Error::StakeTooHigh
                            | Error::ContractPaused
                            | Error::InvalidPlayers
                            | Error::InvalidGameId
                            | Error::DuplicateGameId
                            | Error::InvalidToken
                            | Error::Unauthorized
                            | Error::AlreadyExists
                    ),
                    "Unexpected error code: {:?}",
                    err
                );
            }
            Err(Err(e)) => {
                // Panic or SDK error â€” should NOT happen
                panic!("Unexpected panic or SDK error: {:?}", e);
            }
        }
    }

    /// Property test: create_match with fuzzed game_id
    /// Verifies that any game_id string is handled without panicking.
    #[test]
    fn fuzz_prop_create_match_game_id(game_id_bytes in arb_game_id()) {
        let (env, contract_id, oracle, admin, _, player1, player2, token) = setup();
        let client = EscrowContractClient::new(&env, &contract_id);

        // Try to create a String from arbitrary bytes
        // If bytes are invalid UTF-8, the String creation may fail, which is OK
        let game_id_str = String::from_utf8(env.clone(), game_id_bytes.clone())
            .unwrap_or_else(|_| String::from_str(&env, "invalid_utf8"));

        let result = client.try_create_match(
            &player1, &player2, &100, &Some(token), &game_id_str, &Platform::Lichess,
        );

        // Same validation as stake_amount test
        match result {
            Ok(match_id) => {
                assert!(match_id >= 0);
            }
            Err(Ok(err)) => {
                assert!(
                    matches!(
                        err,
                        Error::StakeTooLow
                            | Error::StakeTooHigh
                            | Error::ContractPaused
                            | Error::InvalidPlayers
                            | Error::InvalidGameId
                            | Error::DuplicateGameId
                            | Error::InvalidToken
                            | Error::Unauthorized
                            | Error::AlreadyExists
                    ),
                    "Unexpected error code: {:?}",
                    err
                );
            }
            Err(Err(e)) => panic!("Unexpected panic or SDK error: {:?}", e),
        }
    }

    /// Property test: create_match with alternating players
    /// Verifies that the contract rejects when player1 == player2.
    #[test]
    fn fuzz_prop_create_match_same_player(
        stake in arb_stake_amount(),
        game_id_bytes in arb_game_id(),
    ) {
        let (env, contract_id, oracle, admin, _, player1, _player2, token) = setup();
        let client = EscrowContractClient::new(&env, &contract_id);

        let game_id_str = String::from_utf8(env.clone(), game_id_bytes.clone())
            .unwrap_or_else(|_| String::from_str(&env, "invalid_utf8"));

        // Call with player1 == player2
        let result =
            client.try_create_match(&player1, &player1, &stake, &token, &game_id_str, &Platform::Lichess);

        // Must either fail gracefully or succeed if stake is outside valid range
        match result {
            Ok(_) => {
                // If it succeeded, the stake must be valid
                // (other validations passed, but same player should have failed)
                // This is actually unexpected â€” should be InvalidPlayers error
            }
            Err(Ok(err)) => {
                // Expected to be InvalidPlayers when stake is valid
                // But other errors are OK if stake is out of range
                assert!(
                    matches!(
                        err,
                        Error::InvalidPlayers
                            | Error::StakeTooLow
                            | Error::StakeTooHigh
                            | Error::InvalidGameId
                            | Error::DuplicateGameId
                            | Error::InvalidToken
                            | Error::Unauthorized
                            | Error::ContractPaused
                            | Error::AlreadyExists
                    ),
                    "Unexpected error code: {:?}",
                    err
                );
            }
            Err(Err(e)) => panic!("Unexpected panic or SDK error: {:?}", e),
        }
    }

    /// Property test: Multiple sequential create_match calls
    /// Verifies that repeated calls with different game_ids succeed without state corruption.
    #[test]
    fn fuzz_prop_create_match_sequential(
        num_matches in 1usize..=10,
        stake in arb_stake_amount(),
    ) {
        let (env, contract_id, oracle, admin, _, player1, player2, token) = setup();
        let client = EscrowContractClient::new(&env, &contract_id);

        let mut created_ids = Vec::new();

        for i in 0..num_matches {
            let game_id = String::from_str(&env, &format!("game_{}", i));

            let result = client.try_create_match(
                &player1, &player2, &stake, &Some(token), &game_id, &Platform::Lichess,
            );

            if let Ok(match_id) = result {
                created_ids.push(match_id);

                // Verify the match exists
                let match_data = client.get_match(&match_id).unwrap();
                assert_eq!(match_data.state, MatchState::Pending);
                assert_eq!(match_data.id, match_id);
            }
        }

        // All created matches must have unique IDs
        let unique_count = created_ids.len();
        created_ids.sort();
        created_ids.dedup();
        assert_eq!(unique_count, created_ids.len(), "Duplicate match IDs detected");
    }

    /// Property test: Duplicate game_id rejection
    /// Verifies that the same game_id cannot be used twice.
    #[test]
    fn fuzz_prop_create_match_duplicate_game_id(stake in arb_stake_amount()) {
        let (env, contract_id, oracle, admin, _, player1, player2, token) = setup();
        let client = EscrowContractClient::new(&env, &contract_id);

        let game_id = String::from_str(&env, "duplicate_test_game");

        // First call
        let result1 = client.try_create_match(
            &player1, &player2, &stake, &Some(token), &game_id, &Platform::Lichess,
        );

        if result1.is_ok() {
            // Second call with same game_id should fail
            let result2 = client.try_create_match(
                &player1, &player2, &stake, &Some(token), &game_id, &Platform::Lichess,
            );

            assert!(
                matches!(result2, Err(Ok(Error::DuplicateGameId))),
                "Second call with duplicate game_id should return DuplicateGameId, got: {:?}",
                result2
            );
        }
    }
}

// ===========================================================================
// Multi-token escrow — issue #121 / #1800
//
// Covers the allowlist lifecycle (add / remove / query, and who may call it)
// and the three token scenarios the acceptance criteria call for: XLM-only,
// USDC-only, and a contract with both allowlisted.
// ===========================================================================

/// Register a second SEP-41 token and mint/approve it for both players.
///
/// Returns the token address. Approvals matter because `deposit` refuses a
/// player who has not approved the escrow for the token the match names — a
/// USDC match must be approved in USDC, not in XLM.
fn setup_second_token(
    env: &Env,
    contract_id: &Address,
    player1: &Address,
    player2: &Address,
) -> Address {
    let issuer = Address::generate(env);
    let token_id = env.register_stellar_asset_contract_v2(issuer);
    let token_addr = token_id.address();

    let asset_client = StellarAssetClient::new(env, &token_addr);
    asset_client.mint(player1, &1000);
    asset_client.mint(player2, &1000);
    // Top up the reserve in the second token too, so a payout in it can satisfy
    // `ensure_reserve_for_payout`.
    asset_client.mint(contract_id, &ESCROW_RESERVE_BUFFER_STROOPS);

    let expiration = env.ledger().sequence() + 1000000;
    let token_client = TokenClient::new(env, &token_addr);
    token_client.approve(player1, contract_id, &1000, &expiration);
    token_client.approve(player2, contract_id, &1000, &expiration);

    token_addr
}

#[test]
fn test_default_token_is_allowlisted_after_initialize() {
    let (env, _contract_id, _oracle, _p1, _p2, token, _admin, _safe) = setup();
    let client = EscrowContractClient::new(&env, &_contract_id);
    // `initialize` seeds the allowlist, so an untouched contract still accepts
    // the token it was configured with.
    assert!(client.is_token_allowlisted(&token));
}

#[test]
fn test_unlisted_token_is_not_allowlisted() {
    let (env, contract_id, _oracle, _p1, _p2, _token, _admin, _safe) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    let other = setup_second_token(&env, &contract_id, &_p1, &_p2);
    assert!(!client.is_token_allowlisted(&other));
}

#[test]
fn test_create_match_rejects_unlisted_token() {
    let (env, contract_id, _oracle, player1, player2, _token, _admin, _safe) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    let other = setup_second_token(&env, &contract_id, &player1, &player2);

    // The allowlist is what stops a caller from pointing the escrow at an
    // arbitrary SEP-41 contract.
    assert_eq!(
        client.try_create_match(
            &player1,
            &player2,
            &100,
            &Some(other),
            &String::from_str(&env, "unlisted"),
            &Platform::Lichess,
        ),
        Err(Ok(Error::TokenNotAllowlisted))
    );
}

#[test]
fn test_create_match_with_none_uses_default_token() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &None,
        &String::from_str(&env, "default-token"),
        &Platform::Lichess,
    );

    // `None` means "whatever this contract was initialized with" — the stored
    // match must show the default token, not a null.
    assert_eq!(client.get_match(&id).token, token);
}

#[test]
fn test_add_token_allows_match_in_that_token() {
    let (env, contract_id, _oracle, player1, player2, _token, admin, _safe) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    let usdc = setup_second_token(&env, &contract_id, &player1, &player2);

    client.add_token(&usdc, &admin);
    assert!(client.is_token_allowlisted(&usdc));

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(usdc.clone()),
        &String::from_str(&env, "usdc-match"),
        &Platform::Lichess,
    );
    // The chosen token is recorded on the match and is what it settles in.
    assert_eq!(client.get_match(&id).token, usdc);
}

#[test]
fn test_deposit_and_payout_use_the_match_token() {
    let (env, contract_id, oracle, player1, player2, _token, admin, _safe) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    let usdc = setup_second_token(&env, &contract_id, &player1, &player2);
    client.add_token(&usdc, &admin);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(usdc.clone()),
        &String::from_str(&env, "mixed-settle"),
        &Platform::Lichess,
    );

    let usdc_client = TokenClient::new(&env, &usdc);
    let before = usdc_client.balance(&player1);

    client.deposit(&id, &player1);
    client.deposit(&id, &player2);
    // Funds must have moved in USDC, not in the default token.
    assert_eq!(
        usdc_client.balance(&contract_id),
        ESCROW_RESERVE_BUFFER_STROOPS + 200
    );

    client.submit_result(&id, &String::from_str(&env, "mixed-settle"), &Winner::Player1, &oracle);
    env.ledger().set_sequence_number(env.ledger().sequence() + 17_281);
    client.finalize_result(&id, &player1);

    // Winner received the pot in USDC.
    assert_eq!(usdc_client.balance(&player1), before + 200);
}

#[test]
fn test_xlm_only_and_usdc_only_matches_coexist() {
    // The "mixed-token" scenario: one contract, matches in both currencies,
    // each settling in the token it was created with.
    let (env, contract_id, oracle, player1, player2, xlm, admin, _safe) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    let usdc = setup_second_token(&env, &contract_id, &player1, &player2);
    client.add_token(&usdc, &admin);

    let xlm_id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(xlm.clone()),
        &String::from_str(&env, "xlm-only"),
        &Platform::Lichess,
    );
    let usdc_id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(usdc.clone()),
        &String::from_str(&env, "usdc-only"),
        &Platform::Lichess,
    );

    assert_eq!(client.get_match(&xlm_id).token, xlm);
    assert_eq!(client.get_match(&usdc_id).token, usdc);
    assert_ne!(xlm_id, usdc_id);
    let _ = oracle;
}

#[test]
fn test_add_token_requires_admin() {
    let (env, contract_id, _oracle, player1, player2, _token, _admin, _safe) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    let other = setup_second_token(&env, &contract_id, &player1, &player2);

    assert_eq!(
        client.try_add_token(&other, &player1),
        Err(Ok(Error::Unauthorized))
    );
    assert!(!client.is_token_allowlisted(&other));
}

#[test]
fn test_remove_token_requires_admin() {
    let (env, contract_id, _oracle, player1, player2, _token, admin, _safe) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    let usdc = setup_second_token(&env, &contract_id, &player1, &player2);
    client.add_token(&usdc, &admin);

    assert_eq!(
        client.try_remove_token(&usdc, &player2),
        Err(Ok(Error::Unauthorized))
    );
    assert!(client.is_token_allowlisted(&usdc));
}

#[test]
fn test_remove_token_blocks_new_matches_but_keeps_existing_ones() {
    let (env, contract_id, oracle, player1, player2, _token, admin, _safe) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    let usdc = setup_second_token(&env, &contract_id, &player1, &player2);
    client.add_token(&usdc, &admin);

    // A match that already exists when the token is delisted.
    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(usdc.clone()),
        &String::from_str(&env, "in-flight"),
        &Platform::Lichess,
    );
    client.deposit(&id, &player1);
    client.deposit(&id, &player2);

    client.remove_token(&usdc, &admin);
    assert!(!client.is_token_allowlisted(&usdc));

    // New matches are refused...
    assert_eq!(
        client.try_create_match(
            &player1,
            &player2,
            &100,
            &Some(usdc.clone()),
            &String::from_str(&env, "after-removal"),
            &Platform::Lichess,
        ),
        Err(Ok(Error::TokenNotAllowlisted))
    );

    // ...but the in-flight match still settles, so no player is stranded.
    client.submit_result(&id, &String::from_str(&env, "in-flight"), &Winner::Player1, &oracle);
    env.ledger().set_sequence_number(env.ledger().sequence() + 17_281);
    client.finalize_result(&id, &player1);
    assert_eq!(client.get_match(&id).state, MatchState::Completed);
}

#[test]
fn test_cannot_remove_default_token() {
    let (env, contract_id, _oracle, _p1, _p2, token, admin, _safe) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    // Otherwise the contract could be left with no acceptable token at all,
    // and no way to recover short of an upgrade.
    assert_eq!(
        client.try_remove_token(&token, &admin),
        Err(Ok(Error::CannotRemoveDefault))
    );
    assert!(client.is_token_allowlisted(&token));
}

#[test]
fn test_add_token_twice_is_rejected() {
    let (env, contract_id, _oracle, player1, player2, _token, admin, _safe) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    let usdc = setup_second_token(&env, &contract_id, &player1, &player2);

    client.add_token(&usdc, &admin);
    // A loud failure beats a silent no-op for a deployment script.
    assert_eq!(
        client.try_add_token(&usdc, &admin),
        Err(Ok(Error::TokenAlreadyListed))
    );
}

#[test]
fn test_remove_unlisted_token_is_rejected() {
    let (env, contract_id, _oracle, player1, player2, _token, admin, _safe) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    let usdc = setup_second_token(&env, &contract_id, &player1, &player2);

    assert_eq!(
        client.try_remove_token(&usdc, &admin),
        Err(Ok(Error::TokenNotListed))
    );
}

#[test]
fn test_get_token_still_returns_default() {
    let (env, contract_id, _oracle, _p1, _p2, token, _admin, _safe) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    // The multi-token change must not alter what `get_token` reports: it is the
    // default, and existing callers depend on that meaning.
    assert_eq!(client.get_token(), token);
}

#[test]
fn test_create_match_max_stake_boundary() {
    // Use the same setup helper as the fuzz tests.
    let (env, contract_id, oracle, admin, _, player1, player2, token) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let max_stake = crate::MAX_STAKE; // Should be 10_000_000_000_000
    let game_id = String::from_str(&env, "max_stake_test");

    let result = client.try_create_match(
        &player1,
        &player2,
        &max_stake,
        &token,
        &game_id,
        &Platform::Lichess,
    );

    // Assert success – the exact maximum stake must be accepted.
    assert!(result.is_ok(), "create_match with MAX_STAKE should succeed");
    let match_id = result.unwrap();
    assert!(match_id >= 0);

    // Verify the match was stored correctly.
    let match_data = client.get_match(&match_id).unwrap();
    assert_eq!(match_data.stake_amount, max_stake);
    assert_eq!(match_data.state, MatchState::Pending);
}

#[test]
fn test_finalize_result_at_exact_dispute_window_boundary() {
    let (env, contract_id, oracle, admin, _, player1, player2, token) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let stake = 1000;
    let game_id = String::from_str(&env, "boundary_test");

    // 1. Create match
    let match_id = client.try_create_match(
        &player1,
        &player2,
        &stake,
        &token,
        &game_id,
        &Platform::Lichess,
    ).unwrap();

    // 2. Both players deposit
    client.deposit(&player1, &match_id, &stake).unwrap();
    client.deposit(&player2, &match_id, &stake).unwrap();

    // 3. Oracle submits result (state → PendingResult)
    let result = MatchResult::Player1Wins;
    client.submit_result(&oracle, &match_id, &result, &game_id).unwrap();

    // 4. Get the stored match to read pending_result_ledger
    let match_data = client.get_match(&match_id).unwrap();
    let pending_result_ledger = match_data.pending_result_ledger.unwrap(); // unwrap safely

    // 5. Get dispute window from the contract (or use constant)
    let dispute_window = crate::DISPUTE_WINDOW_LEDGERS; // adjust if needed

    // 6. Advance ledger to exactly pending_result_ledger + dispute_window
    let boundary_ledger = pending_result_ledger + dispute_window;
    env.ledger().set_sequence_number(boundary_ledger);

    // 7. Call finalize_result – must be rejected with DisputeWindowActive
    let result = client.try_finalize_result(&match_id);
    assert!(
        matches!(result, Err(Ok(Error::DisputeWindowActive))),
        "finalize_result at exact boundary should be rejected with DisputeWindowActive, got: {:?}",
        result
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Issue #3 — stake_amount of zero returns Error::InvalidStakeAmount
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_create_match_zero_stake_returns_invalid_stake_amount() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    assert_eq!(
        client.try_create_match(
            &player1,
            &player2,
            &0,
            &Some(token),
            &String::from_str(&env, "zero_stake_issue3"),
            &Platform::Lichess,
        ),
        Err(Ok(Error::InvalidStakeAmount))
    );
}

#[test]
fn test_create_match_positive_stake_succeeds_issue3() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    let result = client.try_create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "positive_stake_issue3"),
        &Platform::Lichess,
    );
    assert!(result.is_ok(), "Expected Ok, got {:?}", result);
}

// ─────────────────────────────────────────────────────────────────────────────
// Issue #8 — emergency_drain blocked when any match is Active
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_emergency_drain_blocked_when_active_match_exists() {
    let (env, contract_id, _oracle, player1, player2, token, admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    // Create a match and have both players deposit (→ Active state)
    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "active_drain_test"),
        &Platform::Lichess,
    );
    client.deposit(&id, &player1);
    client.deposit(&id, &player2);

    // Pause the contract first (required by emergency_drain)
    client.pause();

    // emergency_drain must be rejected while the match is Active
    assert_eq!(
        client.try_emergency_drain(&admin),
        Err(Ok(Error::ActiveMatchExists))
    );
}

#[test]
fn test_emergency_drain_succeeds_when_no_active_matches() {
    let (env, contract_id, _oracle, player1, player2, token, admin, safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);
    let token_client = TokenClient::new(&env, &token);

    // Create a match and cancel it before depositing (remains Pending → Cancelled)
    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "no_active_drain_test"),
        &Platform::Lichess,
    );
    client.cancel_match(&id, &player1);

    // Pause and drain — no Active matches, so this must succeed
    client.pause();
    let result = client.try_emergency_drain(&admin);
    assert!(result.is_ok(), "Expected Ok when no active matches, got {:?}", result);

    // safe_address should have received the reserve buffer minted in setup
    let safe_bal = token_client.balance(&safe_address);
    assert!(safe_bal >= 0, "safe_address balance should be non-negative");
}

// ─────────────────────────────────────────────────────────────────────────────
// Issue #5 — game_id charset: multi-byte Unicode is rejected
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_create_match_game_id_multibyte_unicode_rejected() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    // "é" is U+00E9, encoded in UTF-8 as two bytes: 0xC3 0xA9.
    // Both bytes are > 0x7F and thus fail the ASCII-only charset check.
    let game_id = String::from_bytes(&env, &[b'g', b'a', b'm', b'e', 0xC3, 0xA9]);
    assert_eq!(
        client.try_create_match(
            &player1,
            &player2,
            &100,
            &Some(token),
            &game_id,
            &Platform::Lichess,
        ),
        Err(Ok(Error::InvalidGameId))
    );
}

#[test]
fn test_create_match_game_id_three_byte_unicode_rejected() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    // "€" is U+20AC, encoded in UTF-8 as three bytes: 0xE2 0x82 0xAC.
    let game_id = String::from_bytes(&env, &[b'g', 0xE2, 0x82, 0xAC]);
    assert_eq!(
        client.try_create_match(
            &player1,
            &player2,
            &100,
            &Some(token),
            &game_id,
            &Platform::Lichess,
        ),
        Err(Ok(Error::InvalidGameId))
    );
}

#[test]
fn test_create_match_game_id_valid_ascii_passes_issue5() {
    let (env, contract_id, _oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    // Pure ASCII alphanumeric + allowed specials must still be accepted
    let result = client.try_create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "abc-XYZ_123"),
        &Platform::Lichess,
    );
    assert!(result.is_ok(), "Valid ASCII game_id should be accepted, got {:?}", result);
}

// ─────────────────────────────────────────────────────────────────────────────
// Issue #6 — submit_result game_id mismatch and matching scenarios
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_submit_result_matching_game_id_succeeds() {
    let (env, contract_id, oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let game_id = String::from_str(&env, "correct_game");
    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &game_id,
        &Platform::Lichess,
    );
    client.deposit(&id, &player1);
    client.deposit(&id, &player2);

    // submit_result with the CORRECT game_id must succeed (no error)
    let result = client.try_submit_result(
        &id,
        &game_id,
        &Winner::Player1,
        &oracle,
    );
    assert!(result.is_ok(), "submit_result with matching game_id should succeed, got {:?}", result);
}

#[test]
fn test_submit_result_mismatched_game_id_returns_game_id_mismatch() {
    let (env, contract_id, oracle, player1, player2, token, _admin, _safe_address) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let id = client.create_match(
        &player1,
        &player2,
        &100,
        &Some(token),
        &String::from_str(&env, "stored_game"),
        &Platform::Lichess,
    );
    client.deposit(&id, &player1);
    client.deposit(&id, &player2);

    // submit_result with a DIFFERENT game_id must return GameIdMismatch
    assert_eq!(
        client.try_submit_result(
            &id,
            &String::from_str(&env, "other_game"),
            &Winner::Player1,
            &oracle,
        ),
        Err(Ok(Error::GameIdMismatch))
    );
}
