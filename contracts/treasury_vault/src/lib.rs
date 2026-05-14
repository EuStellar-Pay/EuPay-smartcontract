//! # EuPay Treasury Vault Contract
//!
//! A multi-token on-chain treasury that employers use to pre-fund payroll.
//! Funds are deposited here and then allocated to individual payroll streams.
//!
//! ## Design principles
//! - **Non-custodial**: tokens never pass through EuPay servers; they move
//!   directly between on-chain contracts via Soroban's token interface.
//! - **Multi-token**: any Stellar asset (XLM, USDC, custom SAC tokens) can
//!   be deposited and tracked independently.
//! - **Allocation model**: employers allocate vault funds to specific stream IDs,
//!   making it easy to audit which streams are covered.
//! - **Emergency drain**: admin can return all funds to a designated address
//!   in the event of a critical vulnerability (last resort).

#![no_std]

use soroban_sdk::{
    contract, contractimpl, contracttype, contracterror,
    token, Address, BytesN, Env, Symbol,
};

// ── Storage TTLs ──────────────────────────────────────────────────────────────
const INSTANCE_BUMP: u32 = 518_400;    // ~30 days
const BALANCE_BUMP:  u32 = 6_307_200;  // ~1 year

// ── Storage keys ──────────────────────────────────────────────────────────────
#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Admin,
    /// Total token units held for a given asset.
    Balance(Address),
    /// How many tokens from a specific asset are earmarked for a stream.
    StreamAlloc { stream_id: u64, token: Address },
}

// ── Error codes ───────────────────────────────────────────────────────────────
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    NotInitialized     = 2,
    Unauthorized       = 3,
    InvalidAmount      = 4,
    InsufficientFunds  = 5,
    InsufficientAlloc  = 6,
}

// ── Contract ──────────────────────────────────────────────────────────────────
#[contract]
pub struct TreasuryVault;

#[contractimpl]
impl TreasuryVault {
    // ── Initialisation ────────────────────────────────────────────────────────

    /// One-time initialisation; sets the vault admin.
    pub fn initialize(env: Env, admin: Address) -> Result<(), Error> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(Error::AlreadyInitialized);
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().extend_ttl(INSTANCE_BUMP, INSTANCE_BUMP);
        Ok(())
    }

    /// Upgrade the contract WASM (admin only).
    pub fn upgrade(env: Env, new_wasm: BytesN<32>) -> Result<(), Error> {
        Self::require_admin(&env)?;
        env.deployer().update_current_contract_wasm(new_wasm);
        Ok(())
    }

    /// Rotate the admin key (requires both old and new admin to sign).
    pub fn set_admin(env: Env, new_admin: Address) -> Result<(), Error> {
        Self::require_admin(&env)?;
        new_admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &new_admin);
        env.storage().instance().extend_ttl(INSTANCE_BUMP, INSTANCE_BUMP);
        Ok(())
    }

    // ── Deposit & Withdrawal ──────────────────────────────────────────────────

    /// Deposit any Stellar asset into the treasury.
    ///
    /// The caller must have pre-approved the transfer via the token contract.
    /// Returns the new total balance for that token.
    pub fn deposit(
        env:    Env,
        from:   Address,
        token:  Address,
        amount: i128,
    ) -> Result<i128, Error> {
        from.require_auth();
        if amount <= 0 { return Err(Error::InvalidAmount); }

        // Transfer tokens from caller into vault
        token::Client::new(&env, &token)
            .transfer(&from, &env.current_contract_address(), &amount);

        let new_balance = Self::add_balance(&env, &token, amount);

        env.events().publish(
            (Symbol::new(&env, "deposited"),),
            (from, token, amount, new_balance),
        );

        Ok(new_balance)
    }

    /// Withdraw tokens from the treasury (admin only).
    ///
    /// Withdrawals are intentionally admin-gated — employers access funds
    /// by allocating to streams, not by withdrawing directly.
    pub fn withdraw(
        env:    Env,
        to:     Address,
        token:  Address,
        amount: i128,
    ) -> Result<i128, Error> {
        Self::require_admin(&env)?;
        if amount <= 0 { return Err(Error::InvalidAmount); }

        let balance = Self::get_balance_raw(&env, &token);
        if amount > balance { return Err(Error::InsufficientFunds); }

        token::Client::new(&env, &token)
            .transfer(&env.current_contract_address(), &to, &amount);

        let new_balance = Self::sub_balance(&env, &token, amount);

        env.events().publish(
            (Symbol::new(&env, "withdrawn"),),
            (to, token, amount, new_balance),
        );

        Ok(new_balance)
    }

    // ── Stream Allocation ────────────────────────────────────────────────────

    /// Earmark treasury funds for a specific payroll stream (employer only).
    ///
    /// This moves tokens from the general treasury balance into a
    /// per-stream bucket. The payroll stream contract then pulls from
    /// this allocation when the stream is created or topped up.
    pub fn allocate(
        env:       Env,
        employer:  Address,
        stream_id: u64,
        token:     Address,
        amount:    i128,
    ) -> Result<i128, Error> {
        employer.require_auth();
        if amount <= 0 { return Err(Error::InvalidAmount); }

        let balance = Self::get_balance_raw(&env, &token);
        if amount > balance { return Err(Error::InsufficientFunds); }

        // Move from general pool to per-stream bucket
        Self::sub_balance(&env, &token, amount);
        let new_alloc = Self::add_alloc(&env, stream_id, &token, amount);

        env.events().publish(
            (Symbol::new(&env, "allocated"),),
            (employer, stream_id, token, amount),
        );

        Ok(new_alloc)
    }

    /// Release allocated funds to the payroll contract (or back to employer).
    ///
    /// Called by the payroll stream contract (or admin) to move earmarked
    /// funds to the stream escrow address. Reduces the stream's allocation.
    pub fn release_alloc(
        env:       Env,
        to:        Address,
        stream_id: u64,
        token:     Address,
        amount:    i128,
    ) -> Result<(), Error> {
        Self::require_admin(&env)?;
        if amount <= 0 { return Err(Error::InvalidAmount); }

        let alloc = Self::get_alloc_raw(&env, stream_id, &token);
        if amount > alloc { return Err(Error::InsufficientAlloc); }

        token::Client::new(&env, &token)
            .transfer(&env.current_contract_address(), &to, &amount);

        Self::sub_alloc(&env, stream_id, &token, amount);

        env.events().publish(
            (Symbol::new(&env, "alloc_released"),),
            (stream_id, to, token, amount),
        );

        Ok(())
    }

    /// Return unused allocation back to the general pool (admin only).
    ///
    /// E.g. when a stream is cancelled and the remaining allocation
    /// should be available for future streams.
    pub fn reclaim_alloc(
        env:       Env,
        stream_id: u64,
        token:     Address,
    ) -> Result<i128, Error> {
        Self::require_admin(&env)?;

        let alloc = Self::get_alloc_raw(&env, stream_id, &token);
        if alloc > 0 {
            Self::sub_alloc(&env, stream_id, &token, alloc);
            Self::add_balance(&env, &token, alloc);
        }

        env.events().publish(
            (Symbol::new(&env, "alloc_reclaimed"),),
            (stream_id, token, alloc),
        );

        Ok(alloc)
    }

    // ── Emergency ────────────────────────────────────────────────────────────

    /// Drain the entire balance of one token to a specified address.
    ///
    /// This is a last-resort function for critical security incidents.
    /// It emits a loud event and zeroes the on-chain balance record.
    pub fn emergency_drain(
        env:   Env,
        to:    Address,
        token: Address,
    ) -> Result<i128, Error> {
        Self::require_admin(&env)?;

        let balance = Self::get_balance_raw(&env, &token);
        if balance > 0 {
            token::Client::new(&env, &token)
                .transfer(&env.current_contract_address(), &to, &balance);
            env.storage().persistent().set(&DataKey::Balance(token.clone()), &0i128);
        }

        env.events().publish(
            (Symbol::new(&env, "EMERGENCY_DRAIN"),),
            (to, token, balance),
        );

        Ok(balance)
    }

    // ── Views ─────────────────────────────────────────────────────────────────

    /// Unallocated balance of a specific token.
    pub fn balance(env: Env, token: Address) -> i128 {
        Self::get_balance_raw(&env, &token)
    }

    /// Tokens allocated (earmarked) for a specific stream.
    pub fn stream_allocation(env: Env, stream_id: u64, token: Address) -> i128 {
        Self::get_alloc_raw(&env, stream_id, &token)
    }

    /// Admin address.
    pub fn get_admin(env: Env) -> Result<Address, Error> {
        env.storage().instance().get(&DataKey::Admin).ok_or(Error::NotInitialized)
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    fn require_admin(env: &Env) -> Result<Address, Error> {
        let admin: Address = env.storage().instance()
            .get(&DataKey::Admin)
            .ok_or(Error::NotInitialized)?;
        admin.require_auth();
        Ok(admin)
    }

    fn get_balance_raw(env: &Env, token: &Address) -> i128 {
        env.storage().persistent()
            .get(&DataKey::Balance(token.clone()))
            .unwrap_or(0)
    }

    fn add_balance(env: &Env, token: &Address, amount: i128) -> i128 {
        let new_val = Self::get_balance_raw(env, token) + amount;
        env.storage().persistent().set(&DataKey::Balance(token.clone()), &new_val);
        env.storage().persistent().extend_ttl(&DataKey::Balance(token.clone()), BALANCE_BUMP, BALANCE_BUMP);
        new_val
    }

    fn sub_balance(env: &Env, token: &Address, amount: i128) -> i128 {
        let new_val = (Self::get_balance_raw(env, token) - amount).max(0);
        env.storage().persistent().set(&DataKey::Balance(token.clone()), &new_val);
        new_val
    }

    fn get_alloc_raw(env: &Env, stream_id: u64, token: &Address) -> i128 {
        env.storage().persistent()
            .get(&DataKey::StreamAlloc { stream_id, token: token.clone() })
            .unwrap_or(0)
    }

    fn add_alloc(env: &Env, stream_id: u64, token: &Address, amount: i128) -> i128 {
        let key = DataKey::StreamAlloc { stream_id, token: token.clone() };
        let new_val = Self::get_alloc_raw(env, stream_id, token) + amount;
        env.storage().persistent().set(&key, &new_val);
        env.storage().persistent().extend_ttl(&key, BALANCE_BUMP, BALANCE_BUMP);
        new_val
    }

    fn sub_alloc(env: &Env, stream_id: u64, token: &Address, amount: i128) -> i128 {
        let key = DataKey::StreamAlloc { stream_id, token: token.clone() };
        let new_val = (Self::get_alloc_raw(env, stream_id, token) - amount).max(0);
        env.storage().persistent().set(&key, &new_val);
        new_val
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{
        testutils::Address as _,
        token::{Client as TokenClient, StellarAssetClient},
        Env,
    };

    fn setup_env() -> (Env, Address, Address, Address, Address) {
        let env = Env::default();
        env.mock_all_auths();

        let admin  = Address::generate(&env);
        let user   = Address::generate(&env);
        let token_admin = Address::generate(&env);
        let token  = env.register_stellar_asset_contract_v2(token_admin.clone()).address();
        StellarAssetClient::new(&env, &token).mint(&user, &10_000_000);

        let vault = env.register(TreasuryVault, ());
        TreasuryVaultClient::new(&env, &vault).initialize(&admin);

        (env, vault, token, admin, user)
    }

    #[test]
    fn test_deposit_and_balance() {
        let (env, vault, token, _admin, user) = setup_env();
        let client = TreasuryVaultClient::new(&env, &vault);

        let new_balance = client.deposit(&user, &token, &1_000_000);
        assert_eq!(new_balance, 1_000_000);
        assert_eq!(client.balance(&token), 1_000_000);

        // Vault actually holds the tokens
        let vault_holding = TokenClient::new(&env, &token).balance(&vault);
        assert_eq!(vault_holding, 1_000_000);
    }

    #[test]
    fn test_withdraw_by_admin() {
        let (env, vault, token, admin, user) = setup_env();
        let client = TreasuryVaultClient::new(&env, &vault);

        client.deposit(&user, &token, &5_000);
        let remaining = client.withdraw(&admin, &token, &2_000);
        assert_eq!(remaining, 3_000);
        assert_eq!(client.balance(&token), 3_000);
    }

    #[test]
    fn test_allocate_and_release() {
        let (env, vault, token, admin, user) = setup_env();
        let client = TreasuryVaultClient::new(&env, &vault);

        client.deposit(&user, &token, &100_000);
        let alloc = client.allocate(&user, &1u64, &token, &40_000);
        assert_eq!(alloc, 40_000);

        // General pool reduced
        assert_eq!(client.balance(&token), 60_000);
        assert_eq!(client.stream_allocation(&1u64, &token), 40_000);

        // Admin releases allocated tokens to a stream escrow address
        let stream_escrow = Address::generate(&env);
        client.release_alloc(&stream_escrow, &1u64, &token, &40_000);

        let escrow_balance = TokenClient::new(&env, &token).balance(&stream_escrow);
        assert_eq!(escrow_balance, 40_000);
        assert_eq!(client.stream_allocation(&1u64, &token), 0);
    }

    #[test]
    fn test_reclaim_unused_alloc() {
        let (env, vault, token, admin, user) = setup_env();
        let client = TreasuryVaultClient::new(&env, &vault);

        client.deposit(&user, &token, &100_000);
        client.allocate(&user, &42u64, &token, &50_000);
        assert_eq!(client.balance(&token), 50_000);

        // Cancel stream — reclaim the allocation
        let reclaimed = client.reclaim_alloc(&42u64, &token);
        assert_eq!(reclaimed, 50_000);
        assert_eq!(client.balance(&token), 100_000);
    }

    #[test]
    fn test_insufficient_funds_rejected() {
        let (env, vault, token, _admin, user) = setup_env();
        let client = TreasuryVaultClient::new(&env, &vault);

        client.deposit(&user, &token, &1_000);
        let result = client.try_withdraw(&Address::generate(&env), &token, &5_000);
        assert!(result.is_err());
    }

    #[test]
    fn test_emergency_drain() {
        let (env, vault, token, admin, user) = setup_env();
        let client = TreasuryVaultClient::new(&env, &vault);

        client.deposit(&user, &token, &999_999);
        let safe_address = Address::generate(&env);
        let drained = client.emergency_drain(&safe_address, &token);

        assert_eq!(drained, 999_999);
        assert_eq!(client.balance(&token), 0);
        let safe_bal = TokenClient::new(&env, &token).balance(&safe_address);
        assert_eq!(safe_bal, 999_999);
    }

    #[test]
    fn test_double_initialize_rejected() {
        let (env, vault, _token, admin, _user) = setup_env();
        let client = TreasuryVaultClient::new(&env, &vault);
        let result = client.try_initialize(&admin);
        assert!(result.is_err());
    }

    #[test]
    fn test_multiple_tokens() {
        let (env, vault, token1, _admin, user) = setup_env();
        let client = TreasuryVaultClient::new(&env, &vault);

        let token2_admin = Address::generate(&env);
        let token2 = env.register_stellar_asset_contract_v2(token2_admin.clone()).address();
        StellarAssetClient::new(&env, &token2).mint(&user, &10_000_000);

        client.deposit(&user, &token1, &300_000);
        client.deposit(&user, &token2, &700_000);

        assert_eq!(client.balance(&token1), 300_000);
        assert_eq!(client.balance(&token2), 700_000);
    }
}
