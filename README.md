# tap-escrow-manager

This service maintains TAP escrow balances on behalf of a gateway sender.

The following data sources are monitored to guide the allocation of GRT into the [TAP Escrow contract](https://github.com/semiotic-ai/timeline-aggregation-protocol-contracts):

- [Graph Network Subgraph](https://github.com/graphprotocol/graph-network-subgraph)
- [TAP Subgraph](https://github.com/semiotic-ai/timeline-aggregation-protocol-subgraph)
- The gateway's Kafka topic for indexer query reports, to account for outstanding debts from receipts sent to indexers
  - Note: Kafka messages are processed for the past 28 days (Escrow `withdrawEscrowThawingPeriod`). Daily aggregations are stored to a file to avoid reprocessing on restart.

# Configuration

Configuration options are set via a single JSON file. The structure of the file is defined in [src/config.rs](src/config.rs).

The sender address used for tap-escrow-manager expects authorizedSigners:
- Sender: Requires ETH for transaction gas and GRT to allocate into TAP escrow balances for paying indexers
- Authorized signer: Used by the gateway and tap-aggregator to sign receipts and RAVs

The tap-escrow-manager will automatically setup authorized signers set in the configuration on startup. This requires the secret keys for the authorized signer wallets to be present in the `signers` config field.

## Setting up Authorized Signers Manually

To set up authorized signers for tap-escrow-manager:

1. [Find the escrow-contract address](https://github.com/semiotic-ai/timeline-aggregation-protocol-contracts/blob/main/addresses.json) for your network.
2. Navigate to the relevant blockchain explorer (e.g., https://sepolia.arbiscan.io/address/0x1e4dC4f9F95E102635D8F7ED71c5CdbFa20e2d02).
3. Connect the sender address (the address tap-escrow-manager is running with, or the address whose private key you provide in the `secret_key` field of `tap-escrow-manager` config).
4. Go to the "Write Contract" tab and find the `authorizeSigner` function.
5. Generate proof and proofDeadline using the script below.

```bash
mkdir proof-generator && cd proof-generator
npm init -y
npm install ethers
cat > generateProof.js << EOL
const ethers = require('ethers');

async function generateProof(signerPrivateKey, proofDeadline, senderAddress, chainId) {
    const signer = new ethers.Wallet(signerPrivateKey);
    
    const messageHash = ethers.solidityPackedKeccak256(
        ['uint256', 'uint256', 'address'],
        [chainId, proofDeadline, senderAddress]
    );

    const digest = ethers.hashMessage(ethers.getBytes(messageHash));
    const signature = await signer.signMessage(ethers.getBytes(messageHash));

    return signature;
}

const signerPrivateKey = process.argv[2];
const senderAddress = process.argv[3];
const chainId = parseInt(process.argv[4]);
const proofDeadline = Math.floor(Date.now() / 1000) + 3600; // 1 hour from now

if (!signerPrivateKey || !senderAddress || !chainId) {
    console.error('Usage: node generateProof.js <signerPrivateKey> <senderAddress> <chainId>');
    process.exit(1);
}

generateProof(signerPrivateKey, proofDeadline, senderAddress, chainId)
    .then(proof => {
        console.log('Proof:', proof);
        console.log('ProofDeadline:', proofDeadline);
        console.log('Human-readable date:', new Date(proofDeadline * 1000).toUTCString());
        console.log('Chain ID:', chainId);
    })
    .catch(error => console.error('Error:', error));
EOL

echo "Setup complete. Run the script with:"
echo "node generateProof.js <authorizedSignerPrivateKey> <senderAddress> <chainId>"
```
6. Pass signerAddress, proofDeadline, and proof to the contract and sign the transaction. Repeat if using multiple authorisedSigners

# Logs

Log levels are controlled by the `RUST_LOG` environment variable ([details](https://docs.rs/env_logger/latest/env_logger/#enabling-logging)).

example: `RUST_LOG=info,tap_escrow_manager=debug cargo run -- config.json`

# Useful Commands

- `rpk group seek tap-escrow-manager-mainnet --to $unix_timestamp`
