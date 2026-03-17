#![no_std]
use soroban_sdk::{contract, contractimpl, contracttype, token, Address, Env, Symbol};

#[contracttype] pub enum DataKey { Admin, Balance, Token }
#[contracterror] #[derive(Copy,Clone,Debug,Eq,PartialEq)] #[repr(u32)]
pub enum VaultError { NotAdmin=1, InsufficientFunds=2, InvalidAmount=3 }

#[contract] pub struct TreasuryVault;

#[contractimpl]
impl TreasuryVault {
  pub fn initialize(env: Env, admin: Address, token: Address) {
    admin.require_auth();
    env.storage().instance().set(&DataKey::Admin, &admin);
    env.storage().instance().set(&DataKey::Token, &token);
    env.storage().instance().set(&DataKey::Balance, &0i128);
  }
  pub fn deposit(env: Env, from: Address, amount: i128) -> Result<i128, VaultError> {
    from.require_auth();
    if amount <= 0 { return Err(VaultError::InvalidAmount); }
    let token: Address = env.storage().instance().get(&DataKey::Token).unwrap();
    let token_client = token::Client::new(&env, &token);
    token_client.transfer(&from, &env.current_contract_address(), &amount);
    let balance: i128 = env.storage().instance().get(&DataKey::Balance).unwrap_or(0);
    let new_balance = balance + amount;
    env.storage().instance().set(&DataKey::Balance, &new_balance);
    env.events().publish((Symbol::new(&env,"deposited"),),(from, amount));
    Ok(new_balance)
  }
  pub fn withdraw(env: Env, to: Address, amount: i128) -> Result<i128, VaultError> {
    let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
    admin.require_auth();
    if amount <= 0 { return Err(VaultError::InvalidAmount); }
    let balance: i128 = env.storage().instance().get(&DataKey::Balance).unwrap_or(0);
    if amount > balance { return Err(VaultError::InsufficientFunds); }
    let token: Address = env.storage().instance().get(&DataKey::Token).unwrap();
    let token_client = token::Client::new(&env, &token);
    token_client.transfer(&env.current_contract_address(), &to, &amount);
    let new_balance = balance - amount;
    env.storage().instance().set(&DataKey::Balance, &new_balance);
    Ok(new_balance)
  }
  pub fn balance(env: Env) -> i128 { env.storage().instance().get(&DataKey::Balance).unwrap_or(0) }
}
