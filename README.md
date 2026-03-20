# EuPay Smart Contracts

<div align="center">

![EuPay Contracts](https://img.shields.io/badge/EuPay-Smart%20Contracts-ef4444?style=for-the-badge)

**Soroban Payroll Streaming Contracts on Stellar**

[![License](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](LICENSE)
[![Stellar](https://img.shields.io/badge/Built%20on-Stellar-7D00FF?logo=stellar)](https://stellar.org)
[![Soroban](https://img.shields.io/badge/Smart%20Contracts-Soroban-00D4FF)](https://soroban.stellar.org)
[![Rust](https://img.shields.io/badge/Rust-1.79+-000000?logo=rust)](https://rust-lang.org)

[Overview](#-overview) • [Contracts](#-contracts) • [Quick Start](#-quick-start) • [Security](#-security) • [Contributing](#-contributing)

</div>

---

## 📖 Overview

EuPay Smart Contracts are the on-chain backbone of the EuPay payroll protocol, built with **Soroban** on the Stellar blockchain. They enforce continuous salary streaming, treasury custody, and solvency guarantees — entirely on-chain with no intermediaries.

---

## 📋 Contracts

| Contract | Purpose | Status |
|----------|---------|--------|
| **PayrollStream** | Continuous salary streaming & per-second accrual | ✅ Complete |
| **TreasuryVault** | Employer fund custody with solvency accounting | ✅ Complete |
| **WorkforceRegistry** | Worker profiles & payment preferences | 📋 Planned |
| **AutomationGateway** | AI agent authorization & execution routing | 📋 Planned |

---

### PayrollStream

The core contract powering EuPay. An employer creates a stream with a `rate_per_second`, and the worker can claim accrued earnings at any time.

```
create_stream(employer, worker, token, rate_per_second, duration_ledgers) → stream_id
claim(worker, stream_id) → amount_claimed
cancel_stream(employer, stream_id)
get_stream(stream_id) → PayrollStream
```

### TreasuryVault

Employer fund custody contract. Employers deposit payroll funds, the contract enforces that withdrawals never exceed the balance.

```
initialize(admin, token)
deposit(from, amount) → new_balance
withdraw(to, amount) → new_balance
balance() → i128
```

---

## 🚀 Quick Start

### Prerequisites

- [Rust](https://www.rust-lang.org/tools/install) 1.79+
- [Stellar CLI](https://developers.stellar.org/docs/build/smart-contracts/getting-started/setup#install-the-stellar-cli)
- Wasm target: `rustup target add wasm32-unknown-unknown`

### Build

```bash
# Clone the repository
git clone https://github.com/EuStellar-Pay/EuPay-smartcontract.git
cd EuPay-smartcontract

# Build all contracts
cargo build --target wasm32-unknown-unknown --release
```

### Test

```bash
cargo test
```

### Deploy to Testnet

```bash
# Configure Stellar CLI identity
stellar keys generate --network testnet deployer

# Deploy PayrollStream
stellar contract deploy \
  --wasm target/wasm32-unknown-unknown/release/eupay_payroll_stream.wasm \
  --source deployer \
  --network testnet

# Deploy TreasuryVault
stellar contract deploy \
  --wasm target/wasm32-unknown-unknown/release/eupay_treasury_vault.wasm \
  --source deployer \
  --network testnet
```

---

## 🏗️ Architecture

```
contracts/
├── payroll_stream/         # Continuous salary streaming
│   └── src/lib.rs         # Stream create / claim / cancel
└── treasury_vault/         # Employer fund custody
    └── src/lib.rs         # Deposit / withdraw / balance
```

### How Streaming Works

```
Employer deposits → TreasuryVault
Employer creates  → PayrollStream (rate/s, duration)
Worker claims     → Earned = elapsed_ledgers × rate_per_second
                    Transfer from contract to worker wallet
```

---

## 🔒 Security

- **Solvency Invariants** — Treasury balance ≥ committed stream obligations enforced on-chain
- **Authorization Checks** — All fund movements require `require_auth()` from the appropriate party
- **Overflow Protection** — Rust's checked arithmetic; `overflow-checks = true` in release profile
- **Reentrancy Safe** — Soroban's execution model prevents classic EVM-style reentrancy

---

## 📊 Roadmap

| Phase | Contract | Timeline | Status |
|-------|----------|----------|--------|
| Phase 1 | PayrollStream + TreasuryVault | Q1 2026 | ✅ Complete |
| Phase 2 | WorkforceRegistry | Q2 2026 | 📋 Planned |
| Phase 3 | AutomationGateway | Q3 2026 | 📋 Planned |
| Phase 4 | Security Audit | Q4 2026 | 📋 Planned |

---

## 🤝 Contributing

See [Contributing Guide](../CONTRIBUTING.md). Minimum test coverage: 90%.

---

## 📜 License

Apache 2.0 — see [LICENSE](LICENSE)

---

## 👥 Past Contributors

| GitHub | Role |
|--------|------|
| [@Uchechukwu-Ekezie](https://github.com/Uchechukwu-Ekezie) | Past Contributor |
| [@bakarezainab](https://github.com/bakarezainab) | Past Contributor |
| [@Gbangbolaoluwagbemiga](https://github.com/Gbangbolaoluwagbemiga) | Past Contributor |
| [@Wilfred007](https://github.com/Wilfred007) | Past Contributor |
| [@meshackyaro](https://github.com/meshackyaro) | Past Contributor |
| [@ogazboiz](https://github.com/ogazboiz) | Past Contributor |
| [@Godbrand0](https://github.com/Godbrand0) | Past Contributor |
| [@Christopherdominic](https://github.com/Christopherdominic) | Past Contributor |
| [@Olowodarey](https://github.com/Olowodarey) | Past Contributor |
| [@emdevelopa](https://github.com/emdevelopa) | Past Contributor |
| [@pope-h](https://github.com/pope-h) | Past Contributor |
| [@DeborahOlaboye](https://github.com/DeborahOlaboye) | Past Contributor |
| [@Rampop01](https://github.com/Rampop01) | Past Contributor |
| [@LaGodxy](https://github.com/LaGodxy) | Past Contributor |
| [@AbelOsaretin](https://github.com/AbelOsaretin) | Past Contributor |
| [@7maylord](https://github.com/7maylord) | Past Contributor |
| [@Jayy4rl](https://github.com/Jayy4rl) | Past Contributor |
| [@CMI-James](https://github.com/CMI-James) | Past Contributor |
| [@edehvictor](https://github.com/edehvictor) | Past Contributor |

<div align="center">

**Built with ❤️ on Stellar**

[EuStellar-Pay Organization](https://github.com/EuStellar-Pay) • [Frontend](https://github.com/EuStellar-Pay/EuPay-frontend) • [Backend](https://github.com/EuStellar-Pay/EuPay-backend) • [Mobile](https://github.com/EuStellar-Pay/EuPay-mobile)

</div>
