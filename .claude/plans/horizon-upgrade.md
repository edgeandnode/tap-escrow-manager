## Horizon Upgrade Engineering Plan

### Overview
Update tap-escrow-manager to work with the Horizon protocol upgrade, which includes changes to the Graph Network Subgraph, deprecation of TAP Escrow Subgraph, and a new TAP Escrow Contract.

### High-Level Changes

**Components Being Updated:**
1. Graph Network Subgraph - Upgraded with new schema
2. TAP Escrow Subgraph - Deprecated (functionality merged into Graph Network Subgraph)
3. TAP Escrow Contract - New contract with updated interfaces

**Components Unchanged:**
- ERC20/GRT Contract
- Kafka infrastructure and message formats
- Core business logic

### Task Breakdown

**Phase 1: Research & Analysis**
1. **Analyze Graph Network Subgraph changes**
   - Understand new schema and entities
   - Identify how escrow data is now stored
   
2. **Map TAP Escrow Subgraph functionality to Graph Network Subgraph**
   - Map `authorized_signers()` query to new schema
   - Map `escrow_accounts()` query to new schema
   
3. **Analyze new TAP Escrow Contract interface changes**
   - Document new method signatures
   - Identify parameter and return type changes
   - Note any new required methods

4. **Design migration strategy for existing escrow balances**
   - Determine if old escrow contract remains active
   - Identify if funds need to be migrated from old to new contract
   - Plan for handling in-flight transactions during migration
   - Define rollback strategy if issues occur
   - Consider dual-contract support period
   - Document operator actions required during migration

**Phase 2: Implementation**
5. **Update subgraph queries in subgraphs.rs**
   - Rewrite `authorized_signers()` function
   - Rewrite `escrow_accounts()` function
   - Update `active_allocations()` if needed
   
6. **Update contract ABIs and interfaces in contracts.rs**
   - Replace Escrow.abi.json with new ABI
   - Update `authorizeSigner()` method
   - Update `depositMany()` method
   - Update error handling for new contract

**Phase 3: Cleanup**
7. **Remove TAP Escrow Subgraph configuration and dependencies**
   - Remove `escrow_subgraph` from config.rs
   - Remove escrow subgraph client from main.rs
   - Update documentation

**Phase 4: Validation**
8. **Test integration with new contracts and subgraphs**
   - Test all queries against new subgraph
   - Test contract interactions
   - Verify escrow balance calculations
   - End-to-end testing with test data

### Code Impact Summary

- **src/subgraphs.rs** - Major rewrite of 2/3 functions
- **src/contracts.rs** - Update contract interface methods
- **src/config.rs** - Remove escrow_subgraph field
- **src/main.rs** - Remove escrow subgraph client
- **src/abi/Escrow.abi.json** - Replace with new ABI