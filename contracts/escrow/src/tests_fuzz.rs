/// Property-based and fuzz tests for the escrow contract's create_match() function.
/// These tests use proptest strategies to generate a wide range of inputs and
/// verify that the contract's validation logic handles edge cases correctly.

extern crate std;
use super::*;
use proptest::prelude::*;
use soroban_sdk::{
    testutils::{
        storage::Instance as _,
        storage::Persistent as _,
        Address as _,
        Ledger as _,
    },
    token::StellarAssetClient,
    vec, Address, Env, String, Symbol, TryFromVal,
};

// ============================================================================
// Proptest Strategies for Inputs
// ============================================================================

/// Strategy that generates valid stake amounts: [MIN_STAKE, MAX_STAKE]
fn valid_stake_strategy() -> impl Strategy<Value = i128> {
    crate::MIN_STAKE..=crate::MAX_STAKE
}

/// Strategy that generates invalid stake amounts below MIN_STAKE
fn invalid_low_stake_strategy() -> impl Strategy<Value = i128> {
    i128::MIN..crate::MIN_STAKE
}

/// Strategy that generates invalid stake amounts above MAX_STAKE
fn invalid_high_stake_strategy() -> impl Strategy<Value = i128> {
    (crate::MAX_STAKE + 1)..=i128::MAX
}

/// Strategy that generates valid game ID strings: [1, MAX_GAME_ID_LEN] characters from [A-Za-z0-9_-]
fn valid_game_id_string_strategy() -> impl Strategy<Value = std::string::String> {
    prop::string::string_regex("[A-Za-z0-9_-]{1,64}")
        .expect("valid regex")
}

/// Strategy that generates invalid game ID strings that are too long (> MAX_GAME_ID_LEN)
fn oversized_game_id_string_strategy() -> impl Strategy<Value = std::string::String> {
    prop::string::string_regex("[A-Za-z0-9_-]{65,128}")
        .expect("valid regex")
}

/// Strategy that generates invalid game ID strings with forbidden characters
fn invalid_char_game_id_string_strategy() -> impl Strategy<Value = std::string::String> {
    prop::string::string_regex("[!@#$%^&*()+=\\[\\]{};:'\",.<>?/`~\\\\|\\s]{1,64}")
        .expect("valid regex")
}

// ============================================================================
// Test Fixture
// ============================================================================

struct FuzzTestFixture {
    env: Env,
    contract_id: Address,
    oracle: Address,
    token: Address,
    player1: Address,
    player2: Address,
    admin: Address,
    safe_address: Address,
}

impl FuzzTestFixture {
    fn setup() -> Self {
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

        // Fund the contract with the required reserve buffer
        asset_client.mint(&contract_id, &crate::ESCROW_RESERVE_BUFFER_STROOPS);

        FuzzTestFixture {
            env,
            contract_id,
            oracle,
            token: token_addr,
            player1,
            player2,
            admin,
            safe_address,
        }
    }

    fn get_client(&self) -> EscrowContractClient {
        EscrowContractClient::new(&self.env, &self.contract_id)
    }
}

// ============================================================================
// Property Tests for Stake Amount Validation
// ============================================================================

proptest! {
    /// Test that valid stake amounts within [MIN_STAKE, MAX_STAKE] are accepted
    #[test]
    fn prop_valid_stake_in_range(stake in valid_stake_strategy()) {
        let fixture = FuzzTestFixture::setup();
        let client = fixture.get_client();
        let game_id = String::from_str(&fixture.env, "valid_stake_test");

        let result = client.try_create_match(
            &fixture.player1,
            &fixture.player2,
            &stake,
            &fixture.token,
            &game_id,
            &Platform::Lichess,
        );

        prop_assert!(
            result.is_ok(),
            "create_match should accept stake {} within valid range [MIN_STAKE, MAX_STAKE]",
            stake
        );
    }

    /// Test that stakes below MIN_STAKE are rejected with StakeTooLow or InvalidStakeAmount
    #[test]
    fn prop_stake_below_minimum(stake in invalid_low_stake_strategy()) {
        let fixture = FuzzTestFixture::setup();
        let client = fixture.get_client();
        let game_id = String::from_str(&fixture.env, "low_stake_test");

        let result = client.try_create_match(
            &fixture.player1,
            &fixture.player2,
            &stake,
            &fixture.token,
            &game_id,
            &Platform::Lichess,
        );

        // Zero stake returns InvalidStakeAmount (#3); other sub-minimum values return StakeTooLow.
        prop_assert!(
            matches!(result, Err(Ok(Error::StakeTooLow)) | Err(Ok(Error::InvalidStakeAmount))),
            "create_match should reject stake {} < MIN_STAKE with StakeTooLow or InvalidStakeAmount",
            stake
        );
    }

    /// Test that stakes above MAX_STAKE are rejected with StakeTooHigh
    #[test]
    fn prop_stake_above_maximum(stake in invalid_high_stake_strategy()) {
        let fixture = FuzzTestFixture::setup();
        let client = fixture.get_client();
        let game_id = String::from_str(&fixture.env, "high_stake_test");

        let result = client.try_create_match(
            &fixture.player1,
            &fixture.player2,
            &stake,
            &fixture.token,
            &game_id,
            &Platform::Lichess,
        );

        prop_assert!(
            matches!(result, Err(Ok(Error::StakeTooHigh))),
            "create_match should reject stake {} > MAX_STAKE with StakeTooHigh",
            stake
        );
    }
}

// ============================================================================
// Property Tests for Game ID Validation
// ============================================================================

proptest! {
    /// Test that valid game IDs (alphanumeric, underscore, hyphen) are accepted
    #[test]
    fn prop_valid_game_id(game_id_str in valid_game_id_string_strategy()) {
        let fixture = FuzzTestFixture::setup();
        let client = fixture.get_client();
        let game_id = String::from_str(&fixture.env, &game_id_str);

        let result = client.try_create_match(
            &fixture.player1,
            &fixture.player2,
            &100,
            &fixture.token,
            &game_id,
            &Platform::Lichess,
        );

        prop_assert!(
            result.is_ok(),
            "create_match should accept valid game_id: {}",
            game_id_str
        );
    }

    /// Test that oversized game IDs (> MAX_GAME_ID_LEN) are rejected with InvalidGameId
    #[test]
    fn prop_oversized_game_id(game_id_str in oversized_game_id_string_strategy()) {
        let fixture = FuzzTestFixture::setup();
        let client = fixture.get_client();
        let game_id = String::from_str(&fixture.env, &game_id_str);

        let result = client.try_create_match(
            &fixture.player1,
            &fixture.player2,
            &100,
            &fixture.token,
            &game_id,
            &Platform::Lichess,
        );

        prop_assert!(
            matches!(result, Err(Ok(Error::InvalidGameId))),
            "create_match should reject game_id longer than MAX_GAME_ID_LEN ({})",
            crate::MAX_GAME_ID_LEN
        );
    }

    /// Test that game IDs with invalid characters are rejected with InvalidGameId
    #[test]
    fn prop_invalid_chars_game_id(game_id_str in invalid_char_game_id_string_strategy()) {
        let fixture = FuzzTestFixture::setup();
        let client = fixture.get_client();
        let game_id = String::from_str(&fixture.env, &game_id_str);

        let result = client.try_create_match(
            &fixture.player1,
            &fixture.player2,
            &100,
            &fixture.token,
            &game_id,
            &Platform::Lichess,
        );

        prop_assert!(
            matches!(result, Err(Ok(Error::InvalidGameId))),
            "create_match should reject game_id with invalid characters: {}",
            game_id_str
        );
    }
}

// ============================================================================
// Exhaustive Fuzz Tests: Boundary Conditions
// ============================================================================

/// Test exact boundary values for stake amounts
#[test]
fn fuzz_stake_boundaries() {
    let fixture = FuzzTestFixture::setup();
    let client = fixture.get_client();

    let test_cases = vec![
        (crate::MIN_STAKE, true, "MIN_STAKE"),
        (crate::MIN_STAKE - 1, false, "MIN_STAKE - 1"),
        (crate::MAX_STAKE, true, "MAX_STAKE"),
        (crate::MAX_STAKE + 1, false, "MAX_STAKE + 1"),
        (100, true, "midrange value 100"),
        (1_000_000, true, "midrange value 1M"),
        (0, false, "zero"),
        (-1, false, "negative"),
    ];

    for (stake, should_pass, description) in test_cases {
        let game_id = String::from_str(&fixture.env, &format!("boundary_test_{}", description.replace(" ", "_")));
        let result = client.try_create_match(
            &fixture.player1,
            &fixture.player2,
            &stake,
            &fixture.token,
            &game_id,
            &Platform::Lichess,
        );

        let passed = result.is_ok();
        assert_eq!(
            passed, should_pass,
            "Stake {} ({}): expected {}, got {}",
            stake, description,
            if should_pass { "pass" } else { "fail" },
            if passed { "pass" } else { "fail" }
        );
    }
}

/// Test exact boundary values for game ID lengths
#[test]
fn fuzz_game_id_length_boundaries() {
    let fixture = FuzzTestFixture::setup();
    let client = fixture.get_client();

    // Test case: game ID at exactly MAX_GAME_ID_LEN (should pass)
    let max_len_id = "a".repeat(crate::MAX_GAME_ID_LEN as usize);
    let game_id = String::from_str(&fixture.env, &max_len_id);
    let result = client.try_create_match(
        &fixture.player1,
        &fixture.player2,
        &100,
        &fixture.token,
        &game_id,
        &Platform::Lichess,
    );
    assert!(
        result.is_ok(),
        "game_id at MAX_GAME_ID_LEN ({}) should be accepted",
        crate::MAX_GAME_ID_LEN
    );

    // Test case: game ID one character over MAX_GAME_ID_LEN (should fail)
    let over_max_id = "b".repeat((crate::MAX_GAME_ID_LEN as usize) + 1);
    let game_id = String::from_str(&fixture.env, &over_max_id);
    let result = client.try_create_match(
        &fixture.player1,
        &fixture.player2,
        &100,
        &fixture.token,
        &game_id,
        &Platform::Lichess,
    );
    assert!(
        matches!(result, Err(Ok(Error::InvalidGameId))),
        "game_id one character over MAX_GAME_ID_LEN should be rejected"
    );
}

/// Test comprehensive game ID character validation
#[test]
fn fuzz_game_id_characters() {
    let fixture = FuzzTestFixture::setup();
    let client = fixture.get_client();

    let test_cases: Vec<(&str, bool, &str)> = vec![
        ("", false, "empty string"),
        ("a", true, "single lowercase letter"),
        ("A", true, "single uppercase letter"),
        ("0", true, "single digit"),
        ("_", true, "single underscore"),
        ("-", true, "single hyphen"),
        ("abc123", true, "alphanumeric"),
        ("test_game-123", true, "with underscore and hyphen"),
        ("UPPERCASE", true, "all uppercase"),
        ("lowercase", true, "all lowercase"),
        ("123", true, "all numeric"),
        ("_-_-_", true, "only underscores and hyphens"),
        (&"a".repeat(64), true, "exactly MAX_GAME_ID_LEN (64)"),
        (&"a".repeat(65), false, "one over MAX_GAME_ID_LEN (65)"),
        ("game@id", false, "invalid char: @"),
        ("game#id", false, "invalid char: #"),
        ("game id", false, "invalid char: space"),
        ("game.id", false, "invalid char: dot"),
        ("game/id", false, "invalid char: slash"),
        ("game\\id", false, "invalid char: backslash"),
        ("game\u{00}id", false, "invalid char: null byte"),
    ];

    for (game_id_str, should_pass, description) in test_cases {
        let game_id = String::from_str(&fixture.env, game_id_str);
        let result = client.try_create_match(
            &fixture.player1,
            &fixture.player2,
            &100,
            &fixture.token,
            &game_id,
            &Platform::Lichess,
        );

        let passed = result.is_ok();
        assert_eq!(
            passed, should_pass,
            "Game ID '{}' ({}): expected {}, got {}",
            game_id_str, description,
            if should_pass { "pass" } else { "fail" },
            if passed { "pass" } else { "fail" }
        );
    }
}

/// Test address validation
#[test]
fn fuzz_address_validation() {
    let fixture = FuzzTestFixture::setup();
    let client = fixture.get_client();

    // Test case: identical players should fail with InvalidPlayers
    let same_addr = fixture.player1.clone();
    let game_id = String::from_str(&fixture.env, "identical_players");
    let result = client.try_create_match(
        &same_addr,
        &same_addr,
        &100,
        &fixture.token,
        &game_id,
        &Platform::Lichess,
    );
    assert!(
        matches!(result, Err(Ok(Error::InvalidPlayers))),
        "create_match should reject identical player addresses"
    );

    // Test case: distinct players should succeed
    let player1 = Address::generate(&fixture.env);
    let player2 = Address::generate(&fixture.env);
    let game_id = String::from_str(&fixture.env, "distinct_players_123");
    let result = client.try_create_match(
        &player1,
        &player2,
        &100,
        &fixture.token,
        &game_id,
        &Platform::Lichess,
    );
    assert!(
        result.is_ok(),
        "create_match should accept two distinct valid addresses"
    );
}
