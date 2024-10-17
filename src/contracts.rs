use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use alloy::{
    network::EthereumWallet,
    primitives::{keccak256, Address, BlockNumber, Bytes, U256},
    providers::{self, Provider as _, ProviderBuilder, WalletProvider},
    signers::{local::PrivateKeySigner, SignerSync as _},
    sol_types::SolValue as _,
    transports::http::Http,
};
use anyhow::{anyhow, Context as _};
use reqwest::Url;

use crate::{Escrow::EscrowInstance, ERC20::ERC20Instance};

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
    providers::RootProvider<Http<reqwest::Client>>,
    Http<reqwest::Client>,
    alloy::network::Ethereum,
>;

pub struct Contracts {
    escrow: EscrowInstance<Http<reqwest::Client>, Arc<Provider>>,
    token: ERC20Instance<Http<reqwest::Client>, Arc<Provider>>,
}

impl Contracts {
    pub fn new(sender: PrivateKeySigner, chain_rpc: Url, token: Address, escrow: Address) -> Self {
        let provider = ProviderBuilder::new()
            .with_recommended_fillers()
            .wallet(EthereumWallet::from(sender))
            .on_http(chain_rpc);
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
            ._0
            .try_into()
            .context("result out of bounds")
    }

    pub async fn approve(&self, amount: u128) -> anyhow::Result<()> {
        self.token
            .approve(self.sender(), U256::from(amount))
            .send()
            .await
            .context("approve send")?
            .with_timeout(Some(Duration::from_secs(30)))
            .with_required_confirmations(2)
            .watch()
            .await
            .context("approve")
            .map(|_| ())
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
            .context("deposit send")?
            .with_timeout(Some(Duration::from_secs(30)))
            .with_required_confirmations(2)
            .get_receipt()
            .await
            .context("deposit")?;
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
            .context("authorize signer send")?
            .with_timeout(Some(Duration::from_secs(60)))
            .with_required_confirmations(2)
            .watch()
            .await
            .context("authorize signer")?;

        Ok(())
    }
}
