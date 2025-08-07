## Horizon Upgrade Engineering Plan

### Overview
Update tap-escrow-manager to work with the Horizon protocol upgrade, which includes changes to the Graph Network Subgraph, deprecation of TAP Escrow Subgraph, and a new TAP Escrow Contract.

### High-Level Changes

**Components Being Updated:**
1. Graph Network Subgraph - Upgraded with new schema
2. TAP Escrow Subgraph - Deprecated (functionality merged into Graph Network Subgraph)
3. TAP Escrow Contract - Completely new contract architecture (PaymentsEscrow + GraphTallyCollector)

**Components Unchanged:**
- ERC20/GRT Contract
- Kafka infrastructure and message formats
- Core business logic

### Research & Planning

**Phase 1: Technical Analysis** ✅ **COMPLETE**
1. **Analyze Graph Network Subgraph changes** ✅
   - New entities: `PaymentsEscrowAccount`, `Payer`, `Signer`, `Receiver`, `PaymentsEscrowTransaction`
   - Escrow data now tracked with three-tier structure: payer → collector → receiver
   - Event handlers: `handleDeposit`, `handleWithdraw`, `handleThaw`, `handleCancelThaw`
   
2. **Map TAP Escrow Subgraph functionality to Graph Network Subgraph** ✅
   - `authorized_signers()`: 
     - OLD: `{ sender(id:"<address>") { signers { id } } }`
     - NEW: `{ payer(id:"<address>") { signers { id } } }`
   - `escrow_accounts()`:
     - OLD: `escrowAccounts(where: { sender: "<address>" }) { balance, receiver { id } }`
     - NEW: `paymentsEscrowAccounts(where: { payer: "<address>" }) { balance, receiver { id } }`
   
3. **Analyze new TAP Escrow Contract interface changes** ✅
   - **Architecture Split**: 
     - PaymentsEscrow: Fund management (`deposit`, `depositTo`, `thaw`, `withdraw`)
     - GraphTallyCollector: Authorization & collection (`collect`, signer validation)
   - **Key Functions**:
     - `deposit(address collector, address receiver, uint256 tokens)`
     - `depositTo(address payer, address collector, address receiver, uint256 tokens)`
     - `multicall(bytes[] calldata data)` - Enables batch operations
   - **Breaking Change**: No `depositMany()` - use multicall with multiple `deposit()` calls

**Phase 2: Migration Strategy** ✅ **COMPLETE**
4. **Design migration strategy for existing escrow balances** ✅
   - **Clean Separation Architecture**: 
     - Pre-upgrade receipts → Old TAP Escrow (existing RAV collection)
     - Post-upgrade receipts → New PaymentsEscrow (via GraphTallyCollector)
   - **No Fund Migration**: Existing balances remain in old contract permanently
   - **No Dual-Contract Management**: tap-escrow-manager only manages new contracts
   - **Old Contract Handling**: 
     - Continues operating independently for pre-upgrade RAV collection
     - Manual intervention when needed (top-up if depleted, drain when safe)
     - Future script creation for remaining fund removal
   - **Deployment**: Clean cutover - old system stops, new system starts
   - **Rollback Strategy**: Not applicable - clean architectural separation

### Implementation Phases

**Phase 3: Core Implementation**
5. **Update subgraph queries in subgraphs.rs**
   - `authorized_signers()`: Change entity from `sender` to `payer` in GraphQL query
   - `escrow_accounts()`: 
     - Change entity from `escrowAccounts` to `paymentsEscrowAccounts` 
     - Update where clause: `sender: "<address>"` → `payer: "<address>"`
     - Handle potential new fields: `collector`, `thawingAmount`, `thawEndTimestamp`
   - `active_allocations()`: Verify no changes needed (should remain same)
   
6. **Update contract ABIs and interfaces in contracts.rs**
   - **File Changes**: 
     - Remove: `src/abi/Escrow.abi.json`
     - Add: `src/abi/PaymentsEscrow.abi.json` + `src/abi/GraphTallyCollector.abi.json`
   - **Contract Struct**: Create `Contracts` with both `payments_escrow` and `tally_collector` instances
   - **Batch Deposits**: Implement `deposit_many()` using:
     ```rust
     let calls: Vec<Bytes> = deposits.map(|(receiver, amount)| 
         payments_escrow.deposit(collector, receiver, amount).calldata()
     ).collect();
     payments_escrow.multicall(calls).send().await?
     ```
   - **Authorization**: Move from PaymentsEscrow to GraphTallyCollector interactions
   - **Error Handling**: Handle errors from both contracts separately

**Phase 4: Configuration & Cleanup**
7. **Remove TAP Escrow Subgraph configuration and dependencies**
   - **Config Changes (src/config.rs)**:
     - Remove: `escrow_subgraph: Url` field
     - Remove: `escrow_contract: Address` field  
     - Add: `payments_escrow_contract: Address` field
     - Add: `graph_tally_collector_contract: Address` field
   - **Main Changes (src/main.rs)**:
     - Remove: `escrow_subgraph` client initialization
     - Remove: `authorized_signers(&mut escrow_subgraph, ...)` call
     - Remove: `escrow_accounts(&mut escrow_subgraph, ...)` call
     - Update: All subgraph calls to use `network_subgraph` client only
   - **Documentation**: Update CLAUDE.md with new contract architecture

**Phase 5: Validation**
8. **Test integration with new contracts and subgraphs**
   - Test all queries against new subgraph
   - Test contract interactions
   - Verify escrow balance calculations
   - End-to-end testing with test data

### Code Impact Summary

- **src/subgraphs.rs** - Update entity names in 2/3 functions
- **src/contracts.rs** - Dual contract architecture + multicall implementation
- **src/config.rs** - Remove escrow_subgraph, add two new contract address fields
- **src/main.rs** - Remove escrow subgraph client, initialize two contracts
- **src/abi/** - Replace Escrow.abi.json with PaymentsEscrow.abi.json + GraphTallyCollector.abi.json