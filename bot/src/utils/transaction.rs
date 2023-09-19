use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use solana_client::{
    client_error::{ClientError, ClientErrorKind},
    nonblocking::rpc_client::RpcClient,
    rpc_client::SerializableTransaction,
    rpc_config::{RpcSendTransactionConfig, RpcTransactionConfig},
};
use solana_sdk::{
    address_lookup_table_account::AddressLookupTableAccount,
    commitment_config::CommitmentConfig,
    instruction::Instruction,
    message::{v0::Message, VersionedMessage},
    signature::Signature,
    transaction::{TransactionError, VersionedTransaction},
};
use solana_transaction_status::{UiTransactionEncoding, UiTransactionStatusMeta};
use tokio::time::sleep;

use crate::{args::Wallet, error::Error};

#[derive(Debug)]
pub enum ClientTransactionError {
    UnableToCompile,
    MissingSigner,
    MissingSignature,
    RpcError,
}

impl From<ClientError> for ClientTransactionError {
    fn from(_: ClientError) -> Self {
        Self::RpcError
    }
}

pub async fn build_signed_transaction(
    rpc_client: &Arc<RpcClient>,
    signer: &Arc<Wallet>,
    instructions: &[Instruction],
    address_lookup_tables: &[AddressLookupTableAccount],
) -> Result<VersionedTransaction, ClientTransactionError> {
    let blockhash = rpc_client.get_latest_blockhash().await?;
    let message = Message::try_compile(
        &signer.pubkey,
        instructions,
        address_lookup_tables,
        blockhash,
    )
    .map_err(|_| ClientTransactionError::UnableToCompile)?;

    let tx = VersionedTransaction::try_new(VersionedMessage::V0(message), &[&signer.keypair])
        .map_err(|_| ClientTransactionError::MissingSigner)?;

    tx.sanitize(true)
        .map_err(|_| ClientTransactionError::MissingSignature)?;

    Ok(tx)
}

const POLL_TIMEOUT: Duration = Duration::from_secs(2);
const TX_VALIDITY_DURATION: u64 = 40;

pub enum TransactionResult {
    Success(Signature, UiTransactionStatusMeta),
    Error(Signature, TransactionError),
    Timeout(Signature),
}

impl TransactionResult {
    pub fn is_success(&self) -> bool {
        match self {
            Self::Success(_, _) => true,
            _ => false,
        }
    }

    pub fn is_err(&self) -> bool {
        match self {
            Self::Error(_, _) => true,
            _ => false,
        }
    }
}

pub async fn send_and_confirm_transaction(
    rpc_client: &Arc<RpcClient>,
    tx: &impl SerializableTransaction,
) -> Result<TransactionResult, Error> {
    let signature = rpc_client
        .send_transaction_with_config(
            tx,
            RpcSendTransactionConfig {
                skip_preflight: true,
                max_retries: Some(20),
                ..Default::default()
            },
        )
        .await?;
    println!("Sent transaction: {}", signature);
    let start = Instant::now();

    loop {
        if start.elapsed().as_secs() > TX_VALIDITY_DURATION {
            break Ok(TransactionResult::Timeout(signature));
        }

        sleep(POLL_TIMEOUT).await;
        let res = rpc_client
            .get_transaction_with_config(
                &signature,
                RpcTransactionConfig {
                    encoding: Some(UiTransactionEncoding::Base64),
                    commitment: Some(CommitmentConfig::confirmed()),
                    max_supported_transaction_version: Some(0),
                },
            )
            .await;

        match res {
            Err(e) => match e.kind {
                ClientErrorKind::SerdeJson(_) => {}
                _ => Err(e)?,
            },
            Ok(res) => {
                let meta = res.transaction.meta.ok_or(Error::TransactionError)?;

                if let Some(e) = meta.err {
                    return Ok(TransactionResult::Error(signature, e));
                } else {
                    return Ok(TransactionResult::Success(signature, meta));
                }
            }
        }
    }
}
