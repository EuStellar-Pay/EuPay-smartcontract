#![no_std]
use soroban_sdk::{contract, contractimpl, contracttype, token, Address, Env, Symbol};

#[contracttype] #[derive(Clone)] pub enum DataKey { Stream(u64), Counter, Admin }

#[contracttype] #[derive(Clone, Debug)]
pub struct PayrollStream { pub id: u64, pub employer: Address, pub worker: Address, pub token: Address, pub rate_per_second: i128, pub start_ledger: u32, pub end_ledger: u32, pub total_claimed: i128, pub is_active: bool }

#[contracterror] #[derive(Copy,Clone,Debug,Eq,PartialEq)] #[repr(u32)]
pub enum StreamError { NotFound=1, Unauthorized=2, StreamEnded=3, NothingToClaim=4, InvalidAmount=5 }

#[contract] pub struct PayrollStreamContract;

#[contractimpl]
impl PayrollStreamContract {
  pub fn initialize(env: Env, admin: Address) {
    admin.require_auth();
    env.storage().instance().set(&DataKey::Admin, &admin);
    env.storage().instance().set(&DataKey::Counter, &0u64);
  }
  pub fn create_stream(env: Env, employer: Address, worker: Address, token: Address, rate_per_second: i128, duration_ledgers: u32) -> Result<u64, StreamError> {
    employer.require_auth();
    if rate_per_second <= 0 { return Err(StreamError::InvalidAmount); }
    let counter: u64 = env.storage().instance().get(&DataKey::Counter).unwrap_or(0);
    let id = counter + 1;
    let stream = PayrollStream { id, employer: employer.clone(), worker, token, rate_per_second, start_ledger: env.ledger().sequence(), end_ledger: env.ledger().sequence() + duration_ledgers, total_claimed: 0, is_active: true };
    env.storage().persistent().set(&DataKey::Stream(id), &stream);
    env.storage().instance().set(&DataKey::Counter, &id);
    env.events().publish((Symbol::new(&env,"stream_created"),),(id, employer));
    Ok(id)
  }
  pub fn claim(env: Env, worker: Address, stream_id: u64) -> Result<i128, StreamError> {
    worker.require_auth();
    let mut stream: PayrollStream = env.storage().persistent().get(&DataKey::Stream(stream_id)).ok_or(StreamError::NotFound)?;
    if stream.worker != worker { return Err(StreamError::Unauthorized); }
    if !stream.is_active { return Err(StreamError::StreamEnded); }
    let current = env.ledger().sequence().min(stream.end_ledger);
    let elapsed = (current - stream.start_ledger) as i128;
    let earned = elapsed * stream.rate_per_second;
    let claimable = earned - stream.total_claimed;
    if claimable <= 0 { return Err(StreamError::NothingToClaim); }
    stream.total_claimed += claimable;
    env.storage().persistent().set(&DataKey::Stream(stream_id), &stream);
    let token_client = token::Client::new(&env, &stream.token);
    token_client.transfer(&env.current_contract_address(), &worker, &claimable);
    env.events().publish((Symbol::new(&env,"claimed"),),(stream_id, claimable));
    Ok(claimable)
  }
  pub fn cancel_stream(env: Env, employer: Address, stream_id: u64) -> Result<(), StreamError> {
    employer.require_auth();
    let mut stream: PayrollStream = env.storage().persistent().get(&DataKey::Stream(stream_id)).ok_or(StreamError::NotFound)?;
    if stream.employer != employer { return Err(StreamError::Unauthorized); }
    stream.is_active = false;
    env.storage().persistent().set(&DataKey::Stream(stream_id), &stream);
    Ok(())
  }
  pub fn get_stream(env: Env, stream_id: u64) -> Option<PayrollStream> { env.storage().persistent().get(&DataKey::Stream(stream_id)) }
}

// TODO: add solvency invariant check before every claim
