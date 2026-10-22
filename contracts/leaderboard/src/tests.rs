//! Leaderboard contract tests â€” issue #124 / #1803.
//!
//! The emphasis is on the accounting invariants rather than happy paths: a
//! leaderboard that silently miscounts wins is worse than one that is empty.

#![cfg(test)]

use super::*;
use soroban_sdk::{testutils::Address as _, Address, Env};

struct Fixture {
    env: Env,
    client: LeaderboardContractClient<'static>,
    contract_id: Address,
    admin: Address,
    source: Address,
    player1: Address,
    player2: Address,
    token: Address,
}

fn setup() -> Fixture {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let source = Address::generate(&env);
    let player1 = Address::generate(&env);
    let player2 = Address::generate(&env);
    let token = Address::generate(&env);

    let id = env.register(LeaderboardContract, ());
    let client = LeaderboardContractClient::new(&env, &id);
    client.initialize(&admin, &source);

    Fixture { env, client, contract_id: id, admin, source, player1, player2, token }
}

fn record(f: &Fixture, outcome: MatchOutcome, payout: i128, p1: &Address, p2: &Address) {
    f.client.record_result(p1, p2, &outcome, &payout, &f.token, &f.source);
}

#[test]
fn stats_start_empty() {
    let f = setup();
    let s = f.client.get_stats(&f.player1);
    assert_eq!(s.wins, 0);
    assert_eq!(s.losses, 0);
    assert_eq!(s.draws, 0);
    assert_eq!(s.earnings, 0);
}

#[test]
fn player1_win_credits_winner_and_counts_loss_for_loser() {
    let f = setup();
    record(&f, MatchOutcome::Player1Won, 100, &f.player1, &f.player2);

    let winner = f.client.get_stats(&f.player1);
    assert_eq!(winner.wins, 1);
    assert_eq!(winner.losses, 0);
    assert_eq!(winner.earnings, 100);

    let loser = f.client.get_stats(&f.player2);
    assert_eq!(loser.wins, 0);
    assert_eq!(loser.losses, 1);
    // A loser is paid nothing.
    assert_eq!(loser.earnings, 0);
}

#[test]
fn player2_win_is_not_mirrored() {
    // The asymmetry is the thing worth pinning: a result where the *second*
    // player wins must credit player2, not player1.
    let f = setup();
    record(&f, MatchOutcome::Player2Won, 100, &f.player1, &f.player2);

    assert_eq!(f.client.get_stats(&f.player1).losses, 1);
    assert_eq!(f.client.get_stats(&f.player1).wins, 0);

    let winner = f.client.get_stats(&f.player2);
    assert_eq!(winner.wins, 1);
    assert_eq!(winner.earnings, 100);
}

#[test]
fn draw_counts_for_both_and_credits_nobody() {
    let f = setup();
    record(&f, MatchOutcome::Draw, 100, &f.player1, &f.player2);

    let p1 = f.client.get_stats(&f.player1);
    let p2 = f.client.get_stats(&f.player2);
    assert_eq!(p1.draws, 1);
    assert_eq!(p2.draws, 1);
    // The escrow returned each player's own stake; nobody was paid, so a draw
    // must not be recorded as earnings.
    assert_eq!(p1.earnings, 0);
    assert_eq!(p2.earnings, 0);
    assert_eq!(p1.wins, 0);
    assert_eq!(p2.wins, 0);
}

#[test]
fn earnings_accumulate_across_wins() {
    let f = setup();
    record(&f, MatchOutcome::Player1Won, 100, &f.player1, &f.player2);
    record(&f, MatchOutcome::Player1Won, 250, &f.player1, &f.player2);
    let s = f.client.get_stats(&f.player1);
    assert_eq!(s.wins, 2);
    assert_eq!(s.earnings, 350);
}

#[test]
fn earnings_are_recorded_in_the_match_token() {
    let f = setup();
    let usdc = Address::generate(&f.env);
    f.client.record_result(
        &f.player1,
        &f.player2,
        &MatchOutcome::Player1Won,
        &1_000_000,
        &usdc,
        &f.source,
    );
    // A caller rendering earnings must be able to say which currency the
    // number is in.
    assert_eq!(f.client.get_stats(&f.player1).token, usdc);
}

#[test]
fn matches_played_sums_all_three_counters() {
    let f = setup();
    record(&f, MatchOutcome::Player1Won, 10, &f.player1, &f.player2);
    record(&f, MatchOutcome::Player1Won, 10, &f.player1, &f.player2);
    record(&f, MatchOutcome::Draw, 0, &f.player1, &f.player2);
    assert_eq!(f.client.get_stats(&f.player1).matches_played(), Some(3));
    assert_eq!(f.client.get_stats(&f.player2).matches_played(), Some(3));
}

#[test]
fn rejects_unauthorized_caller() {
    let f = setup();
    let stranger = Address::generate(&f.env);
    // Anyone can call, but only a registered source can write. Without this
    // the leaderboard is a graffiti wall.
    assert_eq!(
        f.client.try_record_result(
            &f.player1,
            &f.player2,
            &MatchOutcome::Player1Won,
            &100,
            &f.token,
            &stranger,
        ),
        Err(Ok(Error::Unauthorized))
    );
}

#[test]
fn rejects_self_match() {
    let f = setup();
    // Would otherwise credit one address both a win and a loss.
    assert_eq!(
        f.client.try_record_result(
            &f.player1,
            &f.player1,
            &MatchOutcome::Player1Won,
            &100,
            &f.token,
            &f.source,
        ),
        Err(Ok(Error::InvalidResult))
    );
}

#[test]
fn rejects_negative_payout() {
    let f = setup();
    assert_eq!(
        f.client.try_record_result(
            &f.player1,
            &f.player2,
            &MatchOutcome::Player1Won,
            &-1,
            &f.token,
            &f.source,
        ),
        Err(Ok(Error::InvalidResult))
    );
}

#[test]
fn paused_contract_rejects_results() {
    let f = setup();
    f.client.set_paused(&true, &f.admin);
    assert!(f.client.is_paused());
    assert_eq!(
        f.client.try_record_result(
            &f.player1,
            &f.player2,
            &MatchOutcome::Player1Won,
            &100,
            &f.token,
            &f.source,
        ),
        Err(Ok(Error::ContractPaused))
    );

    f.client.set_paused(&false, &f.admin);
    assert!(!f.client.is_paused());
}

#[test]
fn only_admin_can_pause() {
    let f = setup();
    assert_eq!(
        f.client.try_set_paused(&true, &f.source),
        Err(Ok(Error::Unauthorized))
    );
}

#[test]
fn additional_source_can_be_added_and_removed() {
    let f = setup();
    let backup = Address::generate(&f.env);
    assert!(!f.client.is_source(&backup));

    f.client.add_source(&backup, &f.admin);
    assert!(f.client.is_source(&backup));
    // The second source can now write.
    f.client.record_result(
        &f.player1,
        &f.player2,
        &MatchOutcome::Player1Won,
        &50,
        &f.token,
        &backup,
    );
    assert_eq!(f.client.get_stats(&f.player1).wins, 1);

    f.client.remove_source(&backup, &f.admin);
    assert!(!f.client.is_source(&backup));
    assert_eq!(
        f.client.try_record_result(
            &f.player1,
            &f.player2,
            &MatchOutcome::Player1Won,
            &50,
            &f.token,
            &backup,
        ),
        Err(Ok(Error::Unauthorized))
    );
}

#[test]
fn removing_a_source_keeps_its_earlier_results() {
    let f = setup();
    let backup = Address::generate(&f.env);
    f.client.add_source(&backup, &f.admin);
    f.client.record_result(
        &f.player1,
        &f.player2,
        &MatchOutcome::Player1Won,
        &70,
        &f.token,
        &backup,
    );
    f.client.remove_source(&backup, &f.admin);

    // Revoking write permission is not a rollback.
    assert_eq!(f.client.get_stats(&f.player1).wins, 1);
    assert_eq!(f.client.get_stats(&f.player1).earnings, 70);
}

#[test]
fn initialize_is_not_repeatable() {
    let f = setup();
    assert_eq!(
        f.client.try_initialize(&f.admin, &f.source),
        Err(Ok(Error::AlreadyInitialized))
    );
}

#[test]
fn result_count_increments_per_result() {
    let f = setup();
    assert_eq!(f.client.result_count(), 0);
    record(&f, MatchOutcome::Player1Won, 10, &f.player1, &f.player2);
    assert_eq!(f.client.result_count(), 1);
    record(&f, MatchOutcome::Draw, 0, &f.player1, &f.player2);
    assert_eq!(f.client.result_count(), 2);
}

#[test]
fn result_recording_rejects_counter_overflow() {
    let f = setup();
    f.env.as_contract(&f.contract_id, || {
        f.env.storage().persistent().set(
            &DataKey::Stats(f.player1.clone()),
            &PlayerStats {
                wins: u32::MAX,
                losses: 0,
                draws: 0,
                earnings: 0,
                token: f.token.clone(),
            },
        );
    });

    assert_eq!(
        f.client.try_record_result(
            &f.player1,
            &f.player2,
            &MatchOutcome::Player1Won,
            &1,
            &f.token,
            &f.source,
        ),
        Err(Ok(Error::Overflow))
    );
}

#[test]
fn result_recording_rejects_earnings_overflow() {
    let f = setup();
    f.env.as_contract(&f.contract_id, || {
        f.env.storage().persistent().set(
            &DataKey::Stats(f.player1.clone()),
            &PlayerStats {
                wins: 0,
                losses: 0,
                draws: 0,
                earnings: i128::MAX,
                token: f.token.clone(),
            },
        );
    });

    assert_eq!(
        f.client.try_record_result(
            &f.player1,
            &f.player2,
            &MatchOutcome::Player1Won,
            &1,
            &f.token,
            &f.source,
        ),
        Err(Ok(Error::Overflow))
    );
}

#[test]
fn result_recording_rejects_result_count_overflow() {
    let f = setup();
    f.env.as_contract(&f.contract_id, || {
        f.env.storage().instance().set(&DataKey::ResultCount, &u32::MAX);
    });

    assert_eq!(
        f.client.try_record_result(
            &f.player1,
            &f.player2,
            &MatchOutcome::Player1Won,
            &1,
            &f.token,
            &f.source,
        ),
        Err(Ok(Error::Overflow))
    );
}

#[test]
fn matches_played_detects_overflow() {
    let f = setup();
    let stats = PlayerStats {
        wins: u32::MAX,
        losses: 1,
        draws: 0,
        earnings: 0,
        token: f.token,
    };

    assert_eq!(stats.matches_played(), None);
}

#[test]
fn record_result_with_settlement_is_idempotent() {
    let f = setup();
    let settlement_id = String::from_str(&f.env, "settlement-42");

    f.client.record_result_with_settlement(
        &f.player1,
        &f.player2,
        &MatchOutcome::Player1Won,
        &10,
        &f.token,
        &settlement_id,
        &f.source,
    );
    assert_eq!(f.client.result_count(), 1);

    f.client.record_result_with_settlement(
        &f.player1,
        &f.player2,
        &MatchOutcome::Player1Won,
        &10,
        &f.token,
        &settlement_id,
        &f.source,
    );
    assert_eq!(f.client.result_count(), 1);
    assert_eq!(f.client.get_stats(&f.player1).wins, 1);
}

#[test]
fn get_admin_returns_the_configured_admin() {
    let f = setup();
    assert_eq!(f.client.get_admin(), f.admin);
}

/// Give `player` exactly `wins` wins with 10 units of earnings each, by
/// playing them against a throwaway opponent.
fn give_wins(f: &Fixture, player: &Address, wins: u32) {
    for i in 0..wins {
        let opponent = Address::generate(&f.env);
        let outcome = if i % 2 == 0 {
            MatchOutcome::Player1Won
        } else {
            MatchOutcome::Player2Won
        };
        f.client.record_result(player, &opponent, &outcome, &10, &f.token, &f.source);
    }
}

#[test]
fn empty_leaderboard_returns_no_rows() {
    let f = setup();
    let board = f.client.get_leaderboard(&10, &0);
    assert_eq!(board.len(), 0);
}

#[test]
fn leaderboard_is_sorted_by_wins_descending() {
    let f = setup();
    let strong = Address::generate(&f.env);
    let mid = Address::generate(&f.env);
    let weak = Address::generate(&f.env);

    give_wins(&f, &strong, 3);
    give_wins(&f, &mid, 2);
    give_wins(&f, &weak, 1);

    let board = f.client.get_leaderboard(&10, &0);
    assert_eq!(board.len(), 3);
    assert_eq!(board.get(0).unwrap().player, strong);
    assert_eq!(board.get(1).unwrap().player, mid);
    assert_eq!(board.get(2).unwrap().player, weak);
    assert_eq!(board.get(0).unwrap().stats.wins, 3);
}

#[test]
fn leaderboard_ties_break_on_earnings() {
    let f = setup();
    let rich = Address::generate(&f.env);
    let poor = Address::generate(&f.env);

    // Same number of wins, different earnings.
    f.client.record_result(&rich, &Address::generate(&f.env), &MatchOutcome::Player1Won, &500, &f.token, &f.source);
    f.client.record_result(&poor, &Address::generate(&f.env), &MatchOutcome::Player1Won, &5, &f.token, &f.source);

    let board = f.client.get_leaderboard(&10, &0);
    assert_eq!(board.get(0).unwrap().player, rich);
    assert_eq!(board.get(1).unwrap().player, poor);
}

#[test]
fn leaderboard_order_is_stable_for_identical_records() {
    // Without the address tie-break, two identical players have no defined
    // order and paging would duplicate one row and skip the other.
    let f = setup();
    let a = Address::generate(&f.env);
    let b = Address::generate(&f.env);
    f.client.record_result(&a, &Address::generate(&f.env), &MatchOutcome::Player1Won, &42, &f.token, &f.source);
    f.client.record_result(&b, &Address::generate(&f.env), &MatchOutcome::Player1Won, &42, &f.token, &f.source);

    let first = f.client.get_leaderboard(&10, &0);
    let second = f.client.get_leaderboard(&10, &0);
    assert_eq!(first.get(0).unwrap().player, second.get(0).unwrap().player);
    assert_eq!(first.get(1).unwrap().player, second.get(1).unwrap().player);
    assert_ne!(first.get(0).unwrap().player, first.get(1).unwrap().player);
}

#[test]
fn leaderboard_paginates_without_gaps_or_duplicates() {
    let f = setup();
    for _ in 0..5 {
        let p = Address::generate(&f.env);
        give_wins(&f, &p, 1);
    }
    assert_eq!(f.client.get_leaderboard(&10, &0).len(), 5);

    // Walking the board page by page must visit every player exactly once.
    let mut seen: Vec<Address> = Vec::new(&f.env);
    let mut offset = 0u32;
    loop {
        let page = f.client.get_leaderboard(&2, &offset);
        if page.is_empty() {
            break;
        }
        for e in page.iter() {
            seen.push_back(e.player.clone());
        }
        offset += 2;
    }
    assert_eq!(seen.len(), 5);

    let mut unique: Vec<Address> = Vec::new(&f.env);
    for p in seen.iter() {
        assert!(!unique.contains(&p), "player appeared twice across pages");
        unique.push_back(p.clone());
    }
}

#[test]
fn leaderboard_last_page_is_short_rather_than_an_error() {
    let f = setup();
    for _ in 0..3 {
        give_wins(&f, &Address::generate(&f.env), 1);
    }
    // 3 players, asking for 2 from offset 2 -> exactly one row left.
    let page = f.client.get_leaderboard(&2, &2);
    assert_eq!(page.len(), 1);
}

#[test]
fn leaderboard_rejects_zero_limit() {
    let f = setup();
    assert_eq!(f.client.try_get_leaderboard(&0, &0), Err(Ok(Error::InvalidRange)));
}

#[test]
fn leaderboard_rejects_offset_past_the_end() {
    let f = setup();
    give_wins(&f, &Address::generate(&f.env), 1);
    // One player exists, so offset 5 is past the end.
    assert_eq!(f.client.try_get_leaderboard(&10, &5), Err(Ok(Error::InvalidRange)));
}

#[test]
fn leaderboard_caps_limit_at_max_page_size() {
    let f = setup();
    for _ in 0..3 {
        give_wins(&f, &Address::generate(&f.env), 1);
    }
    // Asking for more than the cap returns a capped page, not an error and not
    // an oversized response.
    let board = f.client.get_leaderboard(&1_000, &0);
    assert_eq!(board.len(), 3);
    assert!(1_000 > MAX_PAGE_SIZE);
}

#[test]
fn leaderboard_entry_carries_stats_and_token() {
    // A frontend rendering one row should not need a second RPC call.
    let f = setup();
    give_wins(&f, &f.player1, 1);
    let board = f.client.get_leaderboard(&10, &0);
    let entry = board.get(0).unwrap();
    assert_eq!(entry.stats.wins, 1);
    assert_eq!(entry.stats.earnings, 10);
    assert_eq!(entry.stats.token, f.token);
}

#[test]
fn loss_only_players_are_still_ranked() {
    let f = setup();
    // A player who has only ever lost still has a completed match and belongs
    // on the board.
    f.client.record_result(
        &Address::generate(&f.env),
        &f.player1,
        &MatchOutcome::Player1Won,
        &10,
        &f.token,
        &f.source,
    );
    let board = f.client.get_leaderboard(&10, &0);
    assert_eq!(board.len(), 2);
    let mut found = false;
    for e in board.iter() {
        if e.player == f.player1 {
            assert_eq!(e.stats.losses, 1);
            assert_eq!(e.stats.wins, 0);
            found = true;
        }
    }
    assert!(found, "the losing player should be on the leaderboard");
}
