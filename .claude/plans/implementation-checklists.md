# Horizon Upgrade - Implementation Checklists

## Phase 3: Core Implementation

### 3.1 Update Subgraph Queries (src/subgraphs.rs)

**3.1.1 Update `authorized_signers()` function**
- [ ] Change GraphQL query entity from `sender` to `payer`
- [ ] Update query string: `{ sender(id:"...") { signers { id } } }` → `{ payer(id:"...") { signers { id } } }`
- [ ] Test query returns same data structure
- [ ] Verify signer address extraction still works

**3.1.2 Update `escrow_accounts()` function** 
- [ ] Change entity from `escrowAccounts` to `paymentsEscrowAccounts`
- [ ] Update where clause: `sender: "<address>"` → `payer: "<address>"`
- [ ] Handle new fields if needed: `collector`, `thawingAmount`, `thawEndTimestamp`
- [ ] Verify receiver address mapping still works (receiver.id → indexer)
- [ ] Test paginated query functionality works with new entity

**3.1.3 Verify `active_allocations()` function**
- [ ] Test existing query against new Graph Network Subgraph
- [ ] Verify allocation data structure unchanged
- [ ] Update if any schema changes affect allocations (unlikely)

### 3.2 Update Contract Architecture (src/contracts.rs)

**3.2.1 Add new contract ABIs**
- [ ] Download PaymentsEscrow ABI from horizon contracts repo
- [ ] Save as `src/abi/PaymentsEscrow.abi.json`
- [ ] Download GraphTallyCollector ABI from horizon contracts repo  
- [ ] Save as `src/abi/GraphTallyCollector.abi.json`
- [ ] Remove old `src/abi/Escrow.abi.json`

**3.2.2 Update Contracts struct**
- [ ] Add `payments_escrow: PaymentsEscrowInstance<DynProvider>` field
- [ ] Add `tally_collector: GraphTallyCollectorInstance<DynProvider>` field
- [ ] Remove old `escrow: EscrowInstance<DynProvider>` field
- [ ] Update `Contracts::new()` to take both contract addresses
- [ ] Initialize both contract instances in constructor
- [ ] Keep `sender()` method unchanged

**3.2.3 Implement multicall deposit functionality**
- [ ] Replace `deposit_many()` implementation with multicall approach
- [ ] Create deposit calls: `payments_escrow.deposit(tally_collector_address, receiver, amount).calldata()`
- [ ] Build `Vec<Bytes>` from all deposit calls
- [ ] Execute `payments_escrow.multicall(calls).send().await`
- [ ] Handle multicall-specific errors (revert on any failed deposit)
- [ ] Test batch deposit functionality with multiple receivers

**3.2.4 Update signer authorization**
- [ ] Remove `authorize_signer()` method from PaymentsEscrow logic
- [ ] Implement signer authorization using GraphTallyCollector
- [ ] Research GraphTallyCollector authorization method signature
- [ ] Update authorization flow in main.rs to use new contract
- [ ] Handle GraphTallyCollector-specific errors
- [ ] Test signer authorization end-to-end

**3.2.5 Update utility methods**
- [ ] Keep `allowance()` method (still uses GRT contract)
- [ ] Keep `approve()` method (still uses GRT contract, approves PaymentsEscrow)
- [ ] Update `approve()` to use PaymentsEscrow address instead of old escrow
- [ ] Add error handling for both new contracts
- [ ] Test GRT allowance and approval flow

## Phase 4: Configuration & Cleanup

### 4.1 Update Configuration (src/config.rs)

**4.1.1 Remove deprecated fields**
- [ ] Remove `escrow_subgraph: Url` field from Config struct
- [ ] Remove `escrow_contract: Address` field from Config struct

**4.1.2 Add new contract fields**
- [ ] Add `payments_escrow_contract: Address` field to Config struct
- [ ] Add `graph_tally_collector_contract: Address` field to Config struct
- [ ] Update serde derives if needed

**4.1.3 Test configuration parsing**
- [ ] Create test config JSON with new fields and without old fields
- [ ] Test config deserializes correctly
- [ ] Test error handling for missing required fields
- [ ] Update any example configs or documentation

### 4.2 Update Main Application (src/main.rs)

**4.2.1 Remove escrow subgraph dependencies**
- [ ] Remove `escrow_subgraph` client initialization
- [ ] Remove `authorized_signers(&mut escrow_subgraph, &contracts.sender())` call
- [ ] Remove `escrow_accounts(&mut escrow_subgraph, &contracts.sender())` call
- [ ] Update both calls to use `network_subgraph` client instead

**4.2.2 Update contract initialization**
- [ ] Update `Contracts::new()` call to pass both contract addresses
- [ ] Pass `config.payments_escrow_contract` and `config.graph_tally_collector_contract`
- [ ] Test contract initialization with both addresses
- [ ] Verify both contract instances are accessible

**4.2.3 Update authorization flow**
- [ ] Update signer authorization to use GraphTallyCollector instead of escrow
- [ ] Test authorization still works during startup if `config.authorize_signers` is true
- [ ] Verify error handling for authorization failures

**4.2.4 Test main loop**
- [ ] Verify escrow account queries work with `network_subgraph`
- [ ] Test deposit operations use new multicall approach  
- [ ] Ensure all business logic (debt calculation, balance adjustments) unchanged

### 4.3 Update Documentation

**4.3.1 Update CLAUDE.md**
- [ ] Update "Components Being Updated" section with dual-contract architecture
- [ ] Update Kafka inputs section (no changes needed)
- [ ] Update subgraph inputs with entity name changes
- [ ] Update blockchain outputs with new contract methods
- [ ] Remove TAP Escrow Subgraph references entirely
- [ ] Add PaymentsEscrow + GraphTallyCollector architecture explanation

## Phase 5: Validation

### 5.1 Unit Testing
- [ ] Test `authorized_signers()` with mock `payer` entity response
- [ ] Test `escrow_accounts()` with mock `paymentsEscrowAccounts` response  
- [ ] Test contract multicall deposit logic with mock contract responses
- [ ] Test configuration parsing with valid and invalid configs
- [ ] Test error handling for both PaymentsEscrow and GraphTallyCollector

### 5.2 Integration Testing (if testnet available)
- [ ] Test subgraph queries against live Horizon Graph Network Subgraph
- [ ] Test PaymentsEscrow deposit operations with testnet contracts
- [ ] Test GraphTallyCollector authorization with testnet contracts
- [ ] Verify GRT allowance/approval flow works with PaymentsEscrow
- [ ] Test complete startup sequence with new configuration

### 5.3 End-to-End Testing
- [ ] Full service startup with Horizon configuration
- [ ] Monitor Kafka message consumption (should be unchanged)
- [ ] Verify debt calculations work correctly
- [ ] Test deposit transactions execute and confirm on-chain
- [ ] Test service handles errors gracefully
- [ ] Performance test with realistic transaction volumes
- [ ] Test service shutdown and restart cycles

### 5.4 Pre-Deployment Verification  
- [ ] Code review focusing on contract interaction changes
- [ ] Run linting: `cargo clippy -- -Dwarnings`
- [ ] Run formatting: `cargo +nightly fmt`
- [ ] Run all tests: `cargo test`
- [ ] Build release binary: `cargo build --release`
- [ ] Verify no changes to core escrow balance logic
- [ ] Final documentation review
- [ ] Deployment runbook preparation