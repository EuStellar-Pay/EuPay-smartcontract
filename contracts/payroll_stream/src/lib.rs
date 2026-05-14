//! # EuPay Payroll Stream Contract
//!
//! A Soroban smart contract that streams salary from employer to worker
//! in real time, measured in token-units per second using the ledger timestamp.
//!
//! ## Architecture
//! - An employer creates a stream, depositing tokens upfront.
//! - The worker can claim earned tokens at any time.
//! - The employer can pause, resume, fund, or cancel at any time.
//! - On cancellation the worker receives earned-but-unclaimed tokens;
//!   the employer receives the unearned remainder.
//! - An admin key can emergency-cancel any stream for dispute resolution.
//!
//! ## Token accounting invariant
//! At all times:  total_funded  >=  total_claimed  +  claimable_now
//!
//! ## Time model
//! We use `env.ledger().timestamp()` (Unix seconds) so that `rate_per_second`
//! is a true per-second rate, independent of ledger cadence.

#![no_std]

use soroban_sdk::{
    contract, contractimpl, contracttype, contracterror,
    token, Address, BytesN, Env, Symbol,
};

// ── Storage TTLs (in ledgers; ~5 s / ledger on Stellar mainnet) ─────────────
/// ~30 days — keep instance alive between admin interactions
const INSTANCE_BUMP: u32 = 518_400;
/// ~1 year  — streams must survive for the full payroll cycle
const STREAM_BUMP: u32 = 6_307_200;

// ── Storage keys ─────────────────────────────────────────────────────────────
#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Admin,
    StreamCounter,
    Stream(u64),
}

// ── Domain types ─────────────────────────────────────────────────────────────
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StreamStatus {
    /// Clock is running; worker accrues tokens every second.
    Active,
    /// Clock is frozen; no new tokens accrue until resumed.
    Paused,
    /// Employer (or admin) cancelled the stream early.
    Cancelled,
    /// Stream ran to its natural end time.
    Completed,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct Stream {
    pub id:                    u64,
    pub employer:              Address,
    pub worker:                Address,
    /// Stellar asset contract address (XLM, USDC, etc.)
    pub token:                 Address,
    /// Token units (stroops for XLM) earned per active second.
    pub rate_per_second:       i128,
    /// Unix timestamp when the stream started.
    pub start_time:            u64,
    /// Unix timestamp when the stream ends (0 = open-ended).
    pub end_time:              u64,
    /// Total tokens deposited to fund this stream.
    pub total_funded:          i128,
    /// Total tokens already paid out to the worker.
    pub total_claimed:         i128,
    pub status:                StreamStatus,
    /// Timestamp when the stream was last paused (0 if not paused).
    pub paused_at:             u64,
    /// Cumulative seconds the stream has spent paused.
    pub total_paused_seconds:  u64,
}

// ── Errors ────────────────────────────────────────────────────────────────────
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    NotInitialized     = 2,
    Unauthorized       = 3,
    NotFound           = 4,
    InvalidRate        = 5,
    InvalidDeposit     = 6,
    InvalidDuration    = 7,
    SelfStream         = 8,
    StreamNotActive    = 9,
    StreamNotPaused    = 10,
    NothingToClaim     = 11,
    InsufficientFunds  = 12,
    AlreadyTerminated  = 13,
}

// ── Contract ──────────────────────────────────────────────────────────────────
#[contract]
pub struct PayrollStream;

#[contractimpl]
impl PayrollStream {
    // ── Initialisation ────────────────────────────────────────────────────────

    /// One-time initialisation; sets the admin key.
    pub fn initialize(env: Env, admin: Address) -> Result<(), Error> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(Error::AlreadyInitialized);
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin,         &admin);
        env.storage().instance().set(&DataKey::StreamCounter, &0u64);
        env.storage().instance().extend_ttl(INSTANCE_BUMP, INSTANCE_BUMP);
        Ok(())
    }

    /// Upgrade the contract WASM (admin only).
    pub fn upgrade(env: Env, new_wasm: BytesN<32>) -> Result<(), Error> {
        Self::require_admin(&env)?;
        env.deployer().update_current_contract_wasm(new_wasm);
        Ok(())
    }

    /// Rotate the admin key (admin only).
    pub fn set_admin(env: Env, new_admin: Address) -> Result<(), Error> {
        Self::require_admin(&env)?;
        new_admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &new_admin);
        env.storage().instance().extend_ttl(INSTANCE_BUMP, INSTANCE_BUMP);
        Ok(())
    }

    // ── Stream lifecycle ──────────────────────────────────────────────────────

    /// Create a new payroll stream.
    ///
    /// # Parameters
    /// - `employer`           — pays into the stream, can pause/cancel.
    /// - `worker`             — receives tokens; must differ from employer.
    /// - `token`              — Stellar asset to stream.
    /// - `rate_per_second`    — token units (e.g. stroops) earned per active second.
    /// - `duration_seconds`   — how long the stream runs; 0 = open-ended.
    /// - `initial_deposit`    — tokens locked upfront (must cover ≥ 1 second).
    ///
    /// Returns the new stream ID.
    pub fn create_stream(
        env:              Env,
        employer:         Address,
        worker:           Address,
        token:            Address,
        rate_per_second:  i128,
        duration_seconds: u64,
        initial_deposit:  i128,
    ) -> Result<u64, Error> {
        employer.require_auth();

        if employer == worker               { return Err(Error::SelfStream);         }
        if rate_per_second <= 0             { return Err(Error::InvalidRate);        }
        if initial_deposit <= 0             { return Err(Error::InvalidDeposit);     }
        if initial_deposit < rate_per_second { return Err(Error::InsufficientFunds); }

        // Pull tokens from employer into contract escrow
        token::Client::new(&env, &token)
            .transfer(&employer, &env.current_contract_address(), &initial_deposit);

        let now = env.ledger().timestamp();
        let end_time = if duration_seconds > 0 { now + duration_seconds } else { 0 };

        // Bump counter
        env.storage().instance().extend_ttl(INSTANCE_BUMP, INSTANCE_BUMP);
        let counter: u64 = env.storage().instance()
            .get(&DataKey::StreamCounter).unwrap_or(0);
        let id = counter + 1;
        env.storage().instance().set(&DataKey::StreamCounter, &id);

        let stream = Stream {
            id,
            employer: employer.clone(),
            worker:   worker.clone(),
            token,
            rate_per_second,
            start_time:           now,
            end_time,
            total_funded:         initial_deposit,
            total_claimed:        0,
            status:               StreamStatus::Active,
            paused_at:            0,
            total_paused_seconds: 0,
        };

        env.storage().persistent().set(&DataKey::Stream(id), &stream);
        env.storage().persistent().extend_ttl(&DataKey::Stream(id), STREAM_BUMP, STREAM_BUMP);

        env.events().publish(
            (Symbol::new(&env, "stream_created"),),
            (id, employer, worker, rate_per_second, initial_deposit),
        );

        Ok(id)
    }

    /// Add more tokens to an existing stream (employer only).
    ///
    /// Useful when a stream is about to run out of funds or the employer
    /// wants to extend an open-ended stream.
    pub fn fund_stream(
        env:       Env,
        employer:  Address,
        stream_id: u64,
        amount:    i128,
    ) -> Result<i128, Error> {
        employer.require_auth();
        if amount <= 0 { return Err(Error::InvalidDeposit); }

        let mut s = Self::load_stream(&env, stream_id)?;
        if s.employer != employer { return Err(Error::Unauthorized); }
        Self::require_live(&s)?;

        token::Client::new(&env, &s.token)
            .transfer(&employer, &env.current_contract_address(), &amount);

        s.total_funded += amount;
        Self::save_stream(&env, &s);

        env.events().publish((Symbol::new(&env, "stream_funded"),), (stream_id, amount));
        Ok(s.total_funded)
    }

    /// Worker claims all tokens earned up to this moment.
    pub fn claim(env: Env, worker: Address, stream_id: u64) -> Result<i128, Error> {
        worker.require_auth();

        let mut s = Self::load_stream(&env, stream_id)?;
        if s.worker != worker { return Err(Error::Unauthorized); }
        if s.status == StreamStatus::Cancelled { return Err(Error::AlreadyTerminated); }

        let payout = Self::safe_claimable(&env, &s);
        if payout == 0 { return Err(Error::NothingToClaim); }

        s.total_claimed += payout;

        // Auto-complete when stream has reached its end
        if s.end_time > 0 && env.ledger().timestamp() >= s.end_time {
            s.status = StreamStatus::Completed;
        }

        Self::save_stream(&env, &s);

        token::Client::new(&env, &s.token)
            .transfer(&env.current_contract_address(), &worker, &payout);

        env.events().publish(
            (Symbol::new(&env, "claimed"),),
            (stream_id, worker, payout),
        );

        Ok(payout)
    }

    /// Freeze the stream clock (employer only).
    ///
    /// While paused, no new tokens accrue. The worker can still claim
    /// whatever was earned before the pause.
    pub fn pause_stream(env: Env, employer: Address, stream_id: u64) -> Result<(), Error> {
        employer.require_auth();

        let mut s = Self::load_stream(&env, stream_id)?;
        if s.employer != employer         { return Err(Error::Unauthorized);    }
        if s.status != StreamStatus::Active { return Err(Error::StreamNotActive); }

        s.status    = StreamStatus::Paused;
        s.paused_at = env.ledger().timestamp();
        Self::save_stream(&env, &s);

        env.events().publish((Symbol::new(&env, "stream_paused"),), (stream_id,));
        Ok(())
    }

    /// Resume a paused stream (employer only).
    pub fn resume_stream(env: Env, employer: Address, stream_id: u64) -> Result<(), Error> {
        employer.require_auth();

        let mut s = Self::load_stream(&env, stream_id)?;
        if s.employer != employer          { return Err(Error::Unauthorized);    }
        if s.status != StreamStatus::Paused { return Err(Error::StreamNotPaused); }

        let now = env.ledger().timestamp();
        s.total_paused_seconds += now.saturating_sub(s.paused_at);
        s.paused_at = 0;
        s.status    = StreamStatus::Active;
        Self::save_stream(&env, &s);

        env.events().publish((Symbol::new(&env, "stream_resumed"),), (stream_id,));
        Ok(())
    }

    /// Cancel a stream early (employer only).
    ///
    /// Settlement:
    /// 1. Worker receives all earned-but-unclaimed tokens immediately.
    /// 2. Employer receives the unearned remainder.
    pub fn cancel_stream(env: Env, employer: Address, stream_id: u64) -> Result<(), Error> {
        employer.require_auth();

        let mut s = Self::load_stream(&env, stream_id)?;
        if s.employer != employer { return Err(Error::Unauthorized);    }
        Self::require_live(&s)?;

        Self::settle_and_cancel(&env, &mut s);
        env.events().publish(
            (Symbol::new(&env, "stream_cancelled"),),
            (stream_id, s.employer.clone()),
        );
        Ok(())
    }

    /// Emergency cancel by admin (e.g. after a dispute ruling).
    pub fn admin_cancel(env: Env, stream_id: u64) -> Result<(), Error> {
        Self::require_admin(&env)?;

        let mut s = Self::load_stream(&env, stream_id)?;
        Self::require_live(&s)?;

        Self::settle_and_cancel(&env, &mut s);
        env.events().publish(
            (Symbol::new(&env, "admin_cancelled"),),
            (stream_id,),
        );
        Ok(())
    }

    // ── Views ─────────────────────────────────────────────────────────────────

    /// Return the full stream struct.
    pub fn get_stream(env: Env, stream_id: u64) -> Option<Stream> {
        env.storage().persistent().get(&DataKey::Stream(stream_id))
    }

    /// How many tokens can the worker claim right now?
    pub fn claimable_amount(env: Env, stream_id: u64) -> Result<i128, Error> {
        let s = Self::load_stream(&env, stream_id)?;
        Ok(Self::safe_claimable(&env, &s))
    }

    /// How many seconds has the stream been actively running (excluding pauses)?
    pub fn active_seconds(env: Env, stream_id: u64) -> Result<u64, Error> {
        let s = Self::load_stream(&env, stream_id)?;
        Ok(Self::elapsed_active_seconds(&env, &s))
    }

    /// Remaining funded tokens not yet earned or claimed.
    pub fn remaining_funds(env: Env, stream_id: u64) -> Result<i128, Error> {
        let s = Self::load_stream(&env, stream_id)?;
        let claimable = Self::safe_claimable(&env, &s);
        Ok((s.total_funded - s.total_claimed - claimable).max(0))
    }

    /// Current admin address.
    pub fn get_admin(env: Env) -> Result<Address, Error> {
        env.storage().instance().get(&DataKey::Admin).ok_or(Error::NotInitialized)
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    fn load_stream(env: &Env, id: u64) -> Result<Stream, Error> {
        env.storage().persistent()
            .get(&DataKey::Stream(id))
            .ok_or(Error::NotFound)
    }

    fn save_stream(env: &Env, s: &Stream) {
        env.storage().persistent().set(&DataKey::Stream(s.id), s);
        env.storage().persistent().extend_ttl(&DataKey::Stream(s.id), STREAM_BUMP, STREAM_BUMP);
    }

    fn require_admin(env: &Env) -> Result<Address, Error> {
        let admin: Address = env.storage().instance()
            .get(&DataKey::Admin)
            .ok_or(Error::NotInitialized)?;
        admin.require_auth();
        Ok(admin)
    }

    fn require_live(s: &Stream) -> Result<(), Error> {
        match s.status {
            StreamStatus::Cancelled | StreamStatus::Completed => Err(Error::AlreadyTerminated),
            _ => Ok(()),
        }
    }

    /// Seconds the stream has actively run (paused time excluded).
    fn elapsed_active_seconds(env: &Env, s: &Stream) -> u64 {
        // When paused, freeze time at the moment of pause
        let effective_now = match s.status {
            StreamStatus::Paused => s.paused_at,
            _ => {
                let now = env.ledger().timestamp();
                if s.end_time > 0 { now.min(s.end_time) } else { now }
            }
        };
        effective_now
            .saturating_sub(s.start_time)
            .saturating_sub(s.total_paused_seconds)
    }

    /// Tokens earned but not yet claimed; capped by funded balance.
    fn safe_claimable(env: &Env, s: &Stream) -> i128 {
        let active_secs = Self::elapsed_active_seconds(env, s) as i128;
        let total_earned = active_secs.saturating_mul(s.rate_per_second);
        let claimable    = total_earned.saturating_sub(s.total_claimed).max(0);
        // Hard cap: never pay out more than what was funded
        let headroom     = s.total_funded.saturating_sub(s.total_claimed).max(0);
        claimable.min(headroom)
    }

    /// Shared logic for cancel / admin_cancel: pay worker, refund employer, mark cancelled.
    fn settle_and_cancel(env: &Env, s: &mut Stream) {
        // If still paused, account for pause duration first
        if s.status == StreamStatus::Paused {
            s.total_paused_seconds += env.ledger().timestamp().saturating_sub(s.paused_at);
            s.paused_at = 0;
        }

        // Pay worker their earned-but-unclaimed amount
        let worker_payout = Self::safe_claimable(env, s);
        if worker_payout > 0 {
            token::Client::new(env, &s.token)
                .transfer(&env.current_contract_address(), &s.worker, &worker_payout);
            s.total_claimed += worker_payout;
        }

        // Refund unearned remainder to employer
        let employer_refund = s.total_funded.saturating_sub(s.total_claimed);
        if employer_refund > 0 {
            token::Client::new(env, &s.token)
                .transfer(&env.current_contract_address(), &s.employer, &employer_refund);
        }

        s.status = StreamStatus::Cancelled;
        Self::save_stream(env, s);
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{
        testutils::{Address as _, Ledger},
        token::{Client as TokenClient, StellarAssetClient},
        Env,
    };

    struct TestSetup {
        env:      Env,
        contract: Address,
        token:    Address,
        admin:    Address,
        employer: Address,
        worker:   Address,
    }

    fn setup() -> TestSetup {
        let env = Env::default();
        env.mock_all_auths();

        let admin    = Address::generate(&env);
        let employer = Address::generate(&env);
        let worker   = Address::generate(&env);

        // Deploy a native asset for testing
        let token_admin = Address::generate(&env);
        let token = env.register_stellar_asset_contract_v2(token_admin.clone()).address();
        StellarAssetClient::new(&env, &token).mint(&employer, &1_000_000_0000000);

        let contract = env.register(PayrollStream, ());
        let client   = PayrollStreamClient::new(&env, &contract);
        client.initialize(&admin);

        TestSetup { env, contract, token, admin, employer, worker }
    }

    fn advance_time(env: &Env, seconds: u64) {
        env.ledger().with_mut(|l| {
            l.timestamp += seconds;
            l.sequence_number += (seconds / 5) as u32;
        });
    }

    #[test]
    fn test_create_and_claim() {
        let t = setup();
        let client = PayrollStreamClient::new(&t.env, &t.contract);

        // 10 stroops per second, 1 hour stream, 3600-second deposit
        let rate    = 10_i128;
        let deposit = 36_000_i128; // covers exactly 1 hour
        let id = client.create_stream(&t.employer, &t.worker, &t.token, &rate, &3600, &deposit);
        assert_eq!(id, 1);

        // Advance 100 seconds
        advance_time(&t.env, 100);

        let claimable = client.claimable_amount(&id);
        assert_eq!(claimable, 1000); // 100 * 10

        let paid = client.claim(&t.worker, &id);
        assert_eq!(paid, 1000);

        // Worker's balance reflects claim
        let worker_bal = TokenClient::new(&t.env, &t.token).balance(&t.worker);
        assert_eq!(worker_bal, 1000);

        // Second claim: nothing to claim immediately after
        let result = client.try_claim(&t.worker, &id);
        assert!(result.is_err());
    }

    #[test]
    fn test_pause_and_resume() {
        let t = setup();
        let client = PayrollStreamClient::new(&t.env, &t.contract);

        let id = client.create_stream(&t.employer, &t.worker, &t.token, &10, &3600, &36_000);

        advance_time(&t.env, 100); // earn 1000
        client.pause_stream(&t.employer, &id);

        advance_time(&t.env, 500); // paused — should not earn anything

        client.resume_stream(&t.employer, &id);
        advance_time(&t.env, 50); // earn 500 more

        let claimable = client.claimable_amount(&id);
        assert_eq!(claimable, 1500); // 100 + 50, pause not counted
    }

    #[test]
    fn test_cancel_settles_correctly() {
        let t = setup();
        let client = PayrollStreamClient::new(&t.env, &t.contract);

        let deposit = 1_000_000_i128;
        let rate    = 100_i128;
        let id      = client.create_stream(&t.employer, &t.worker, &t.token, &rate, &0, &deposit);

        advance_time(&t.env, 200); // earned 20_000

        let employer_bal_before = TokenClient::new(&t.env, &t.token).balance(&t.employer);
        client.cancel_stream(&t.employer, &id);

        let worker_bal   = TokenClient::new(&t.env, &t.token).balance(&t.worker);
        let employer_bal = TokenClient::new(&t.env, &t.token).balance(&t.employer);

        assert_eq!(worker_bal, 20_000);
        assert_eq!(employer_bal - employer_bal_before, deposit - 20_000);

        // Can't claim or cancel again
        assert!(client.try_claim(&t.worker, &id).is_err());
        assert!(client.try_cancel_stream(&t.employer, &id).is_err());
    }

    #[test]
    fn test_solvency_invariant_never_violated() {
        let t = setup();
        let client = PayrollStreamClient::new(&t.env, &t.contract);

        // Fund just enough for 10 seconds
        let rate    = 1_000_i128;
        let deposit = 10_000_i128;
        let id      = client.create_stream(&t.employer, &t.worker, &t.token, &rate, &0, &deposit);

        // Advance 100 seconds (far beyond funded capacity)
        advance_time(&t.env, 100);

        // Claimable must be capped at total_funded, not exceed it
        let claimable = client.claimable_amount(&id);
        assert_eq!(claimable, deposit);

        let paid = client.claim(&t.worker, &id);
        assert_eq!(paid, deposit);

        // Nothing left
        let claimable2 = client.claimable_amount(&id);
        assert_eq!(claimable2, 0);
    }

    #[test]
    fn test_fund_stream_extends_runway() {
        let t = setup();
        let client = PayrollStreamClient::new(&t.env, &t.contract);

        let id = client.create_stream(&t.employer, &t.worker, &t.token, &100, &0, &1_000);

        advance_time(&t.env, 5);
        // Add more fuel
        let new_total = client.fund_stream(&t.employer, &id, &9_000);
        assert_eq!(new_total, 10_000);

        advance_time(&t.env, 95);
        let paid = client.claim(&t.worker, &id);
        assert_eq!(paid, 10_000); // 100 seconds * 100 = 10_000
    }

    #[test]
    fn test_admin_cancel() {
        let t = setup();
        let client = PayrollStreamClient::new(&t.env, &t.contract);

        let id = client.create_stream(&t.employer, &t.worker, &t.token, &10, &0, &100_000);
        advance_time(&t.env, 50);

        client.admin_cancel(&id);

        let stream = client.get_stream(&id).unwrap();
        assert_eq!(stream.status, StreamStatus::Cancelled);

        let worker_bal = TokenClient::new(&t.env, &t.token).balance(&t.worker);
        assert_eq!(worker_bal, 500); // 50 seconds * 10
    }

    #[test]
    fn test_self_stream_rejected() {
        let t = setup();
        let client = PayrollStreamClient::new(&t.env, &t.contract);
        let result = client.try_create_stream(&t.employer, &t.employer, &t.token, &10, &3600, &36_000);
        assert!(result.is_err());
    }

    #[test]
    fn test_stream_auto_completes_at_end_time() {
        let t = setup();
        let client = PayrollStreamClient::new(&t.env, &t.contract);

        // 60-second stream
        let id = client.create_stream(&t.employer, &t.worker, &t.token, &10, &60, &600);

        advance_time(&t.env, 120); // past end time

        let claimable = client.claimable_amount(&id);
        assert_eq!(claimable, 600); // capped at funded amount

        client.claim(&t.worker, &id);
        let stream = client.get_stream(&id).unwrap();
        assert_eq!(stream.status, StreamStatus::Completed);
    }
}
