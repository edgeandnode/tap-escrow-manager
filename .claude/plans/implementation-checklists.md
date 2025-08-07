# Horizon Upgrade - Implementation Checklists

## Phase 3: Core Implementation

### 3.1 Update Subgraph Queries (src/subgraphs.rs)

**3.1.1 Update `authorized_signers()` function**
- [x] Change GraphQL query entity from `sender` to `payer`
- [x] Update query string: `{ sender(id:"...") { signers { id } } }` → `{ payer(id:"...") { signers { id } } }`
- [x] Update parameter names from `sender` to `payer`
- [x] Update struct field names to match new schema
- [x] Test query returns same data structure
- [x] Verify signer address extraction still works

**3.1.2 Update `escrow_accounts()` function** 
- [x] Change entity from `escrowAccounts` to `paymentsEscrowAccounts`
- [x] Update where clause: `sender: "<address>"` → `payer: "<address>"`
- [x] Update parameter names from `sender` to `payer`
- [x] Handle new fields if needed: `collector`, `thawingAmount`, `thawEndTimestamp`
- [x] Verify receiver address mapping still works (receiver.id → indexer)
- [x] Test paginated query functionality works with new entity

**3.1.3 Verify `active_allocations()` function**
- [x] Reviewed code - no changes needed
- [x] Already queries network_subgraph (not escrow subgraph)
- [x] Test existing query against new Graph Network Subgraph
- [x] Verify allocation data structure unchanged

**3.1.4 Update sender → payer terminology**
- [x] Update all function parameters in subgraphs.rs
- [x] Update field name in Contracts struct (contracts.rs)
- [x] Update method name from `sender()` to `payer()` (contracts.rs)
- [x] Update variable names in main.rs
- [x] Update comments in config.rs
- [x] Ensure consistency across all files

### 3.2 Update Contract Architecture (src/contracts.rs)

**3.2.1 Add new contract ABIs**
- [x] PaymentsEscrow ABI provided and verified
- [x] Save as `src/abi/PaymentsEscrow.abi.json`
- [x] GraphTallyCollector ABI provided and verified
- [x] Save as `src/abi/GraphTallyCollector.abi.json`
- [x] Remove old `src/abi/Escrow.abi.json`

**3.2.2 Update Contracts struct**
- [x] Add `payments_escrow: PaymentsEscrowInstance<DynProvider>` field
- [x] Add `graph_tally_collector: GraphTallyCollectorInstance<DynProvider>` field
- [x] Remove old `escrow: EscrowInstance<DynProvider>` field
- [x] Update `Contracts::new()` to take both contract addresses
- [x] Initialize both contract instances in constructor
- [x] Added sol! macros for both new contracts
- [x] Update `sender()` method to `payer()` for consistency

**3.2.3 Implement multicall deposit functionality**
- [x] Replace `deposit_many()` implementation with multicall approach
- [x] Create deposit calls: `payments_escrow.deposit(graph_tally_collector_address, receiver, amount).calldata()`
- [x] Build `Vec<Bytes>` from all deposit calls
- [x] Execute `payments_escrow.multicall(calls).send().await`
- [x] Handle multicall-specific errors with PaymentsEscrowErrors
- [x] Test batch deposit functionality with multiple receivers

**3.2.4 Update signer authorization**
- [x] Move `authorize_signer()` from old escrow to GraphTallyCollector
- [x] Implement signer authorization using GraphTallyCollector
- [x] Verified GraphTallyCollector authorization method signature (same as old)
- [x] Update authorization flow to use graph_tally_collector contract
- [x] Handle GraphTallyCollectorErrors instead of EscrowErrors
- [x] Update chain_id retrieval to use graph_tally_collector provider
- [x] Test signer authorization end-to-end

**3.2.5 Update utility methods**
- [x] Keep `allowance()` method (still uses GRT contract)
- [x] Update `allowance()` to check PaymentsEscrow address instead of old escrow
- [x] Keep `approve()` method (still uses GRT contract, approves PaymentsEscrow)
- [x] Update `approve()` to use PaymentsEscrow address instead of old escrow
- [x] Error handling updated for new contract types
- [x] Test GRT allowance and approval flow

## Phase 4: Configuration & Cleanup

### 4.1 Update Configuration (src/config.rs)

**4.1.1 Remove deprecated fields**
- [x] Remove `escrow_subgraph: Url` field from Config struct
- [x] Remove `escrow_contract: Address` field from Config struct

**4.1.2 Add new contract fields**
- [x] Add `payments_escrow_contract: Address` field to Config struct
- [x] Add `graph_tally_collector_contract: Address` field to Config struct
- [x] Update field documentation comments

**4.1.3 Test configuration parsing**
- [x] Create test config JSON with new fields and without old fields
- [x] Test config deserializes correctly
- [x] Test error handling for missing required fields
- [x] Update any example configs or documentation

### 4.2 Update Main Application (src/main.rs)

**4.2.1 Remove escrow subgraph dependencies**
- [x] Remove `escrow_subgraph` client initialization
- [x] Remove `authorized_signers(&mut escrow_subgraph, &contracts.payer())` call
- [x] Remove `escrow_accounts(&mut escrow_subgraph, &contracts.payer())` call
- [x] Update both calls to use `network_subgraph` client instead
- [x] Update function signatures in subgraphs.rs
- [x] Update subgraph client rebuilding logic

**4.2.2 Update contract initialization**
- [x] Update `Contracts::new()` call to pass both contract addresses
- [x] Pass `config.payments_escrow_contract` and `config.graph_tally_collector_contract`
- [x] Test contract initialization with both addresses
- [x] Verify both contract instances are accessible

**4.2.3 Update authorization flow**
- [x] Authorization flow already updated via contracts.rs changes in Phase 3
- [x] Test authorization still works during startup if `config.authorize_signers` is true
- [x] Verify error handling for authorization failures

**4.2.4 Test main loop**
- [x] Verify escrow account queries work with `network_subgraph`
- [x] Test deposit operations use new multicall approach  
- [x] Ensure all business logic (debt calculation, balance adjustments) unchanged

### 4.3 Update Documentation

**4.3.1 Update CLAUDE.md**
- [x] Update subgraph inputs with entity name changes (payer, paymentsEscrowAccounts)
- [x] Update blockchain outputs with new contract methods (multicall, dual contracts)
- [x] Remove TAP Escrow Subgraph references entirely
- [x] Add PaymentsEscrow + GraphTallyCollector architecture explanation
- [x] Document new contract responsibilities and method signatures
- [x] Update configuration field references

## Phase 5: Validation
 
- [x] Code review focusing on contract interaction changes
- [x] Run linting: `cargo clippy -- -Dwarnings`
- [x] Run formatting: `cargo +nightly fmt`
- [x] Run all tests: `cargo test`
- [x] Build release binary: `cargo build --release`
- [x] Verify no changes to core escrow balance logic
- [x] Final documentation review
- [x] Deployment runbook preparation