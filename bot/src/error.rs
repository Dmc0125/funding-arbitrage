use solana_client::client_error::ClientError;

#[derive(Debug)]
pub enum Error {
    UnableToCreateOutputFile,
    UnableToLoadOutputFile,
    UnableToSaveOutputFile,

    UnableToDecode,
    UnableToDeserialize,

    TransactionError,

    RpcError,
}

impl From<ClientError> for Error {
    fn from(_: ClientError) -> Self {
        Self::RpcError
    }
}
