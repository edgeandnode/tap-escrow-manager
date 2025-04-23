use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use alloy::{
    network::EthereumWallet,
    primitives::{keccak256, Address, BlockNumber, Bytes, U256},
    providers::{self, Provider as _, ProviderBuilder, WalletProvider},
    signers::{local::PrivateKeySigner, SignerSync as _},
    sol,
    sol_types::{SolInterface, SolValue as _},
};
use anyhow::{anyhow, Context as _};
use reqwest::Url;

sol!(
    #[allow(missing_docs)]
    #[sol(rpc)]
    ERC20,
    "src/abi/ERC20.abi.json"
);
use ERC20::ERC20Instance;
sol!(
    #[allow(missing_docs)]
    #[sol(rpc)]
    #[derive(Debug)]
    Escrow,
    "src/abi/Escrow.abi.json"
);
use Escrow::{EscrowErrors, EscrowInstance};

// TODO: figure out how to erase this type
type Provider = providers::fillers::FillProvider<
    providers::fillers::JoinFill<
        providers::fillers::JoinFill<
            providers::Identity,
            providers::fillers::JoinFill<
                providers::fillers::GasFiller,
                providers::fillers::JoinFill<
                    providers::fillers::BlobGasFiller,
                    providers::fillers::JoinFill<
                        providers::fillers::NonceFiller,
                        providers::fillers::ChainIdFiller,
                    >,
                >,
            >,
        >,
        providers::fillers::WalletFiller<EthereumWallet>,
    >,
    providers::RootProvider,
>;

pub struct Contracts {
    escrow: EscrowInstance<Arc<Provider>>,
    token: ERC20Instance<Arc<Provider>>,
}

impl Contracts {
    pub fn new(sender: PrivateKeySigner, chain_rpc: Url, token: Address, escrow: Address) -> Self {
        let provider = ProviderBuilder::new()
            .wallet(EthereumWallet::from(sender))
            .connect_http(chain_rpc);
        let provider = Arc::new(provider);
        let escrow = EscrowInstance::new(escrow, provider.clone());
        let token = ERC20Instance::new(token, provider.clone());
        Self { escrow, token }
    }

    pub fn sender(&self) -> Address {
        self.token.provider().default_signer_address()
    }

    pub async fn allowance(&self) -> anyhow::Result<u128> {
        self.token
            .allowance(self.sender(), *self.escrow.address())
            .call()
            .await
            .context("get allowance")?
            .try_into()
            .context("result out of bounds")
    }

    pub async fn approve(&self, amount: u128) -> anyhow::Result<()> {
        self.token
            .approve(*self.escrow.address(), U256::from(amount))
            .send()
            .await?
            .with_timeout(Some(Duration::from_secs(30)))
            .with_required_confirmations(1)
            .watch()
            .await?;
        Ok(())
    }

    pub async fn deposit_many(
        &self,
        deposits: impl IntoIterator<Item = (Address, u128)>,
    ) -> anyhow::Result<BlockNumber> {
        let (receivers, amounts): (Vec<Address>, Vec<U256>) = deposits
            .into_iter()
            .map(|(r, a)| (r, U256::from(a)))
            .unzip();
        let receipt = self
            .escrow
            .depositMany(receivers, amounts)
            .send()
            .await
            .map_err(decoded_err::<EscrowErrors>)?
            .with_timeout(Some(Duration::from_secs(30)))
            .with_required_confirmations(1)
            .get_receipt()
            .await?;
        let block_number = receipt
            .block_number
            .ok_or_else(|| anyhow!("invalid deposit receipt"))?;
        Ok(block_number)
    }

    pub async fn authorize_signer(&self, signer: &PrivateKeySigner) -> anyhow::Result<()> {
        let chain_id = self
            .escrow
            .provider()
            .get_chain_id()
            .await
            .context("get chain ID")?;
        let deadline_offset_s = 60;
        let deadline = U256::from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs()
                + deadline_offset_s,
        );
        let mut proof_message = [0u8; 84];
        proof_message[0..32].copy_from_slice(&U256::from(chain_id).to_be_bytes::<32>());
        proof_message[32..64].copy_from_slice(&deadline.to_be_bytes::<32>());
        proof_message[64..].copy_from_slice(&self.sender().0.abi_encode_packed());
        let hash = keccak256(proof_message);
        let signature = signer
            .sign_message_sync(hash.as_slice())
            .context("sign authorization proof")?;
        let proof: Bytes = signature.as_bytes().into();

        self.escrow
            .authorizeSigner(signer.address(), deadline, proof)
            .send()
            .await
            .map_err(decoded_err::<EscrowErrors>)?
            .with_timeout(Some(Duration::from_secs(60)))
            .with_required_confirmations(1)
            .watch()
            .await?;
        Ok(())
    }
}

fn decoded_err<E: SolInterface + std::fmt::Debug>(err: alloy::contract::Error) -> anyhow::Error {
    match err {
        alloy::contract::Error::TransportError(alloy::transports::RpcError::ErrorResp(err)) => {
            match err.as_decoded_interface_error::<E>() {
                Some(decoded) => anyhow!("{:?}", decoded),
                None => anyhow!(err),
            }
        }
        _ => anyhow!(err),
    }
}
