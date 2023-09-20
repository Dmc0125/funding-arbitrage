use solana_client::client_error::ClientError;

use crate::utils::websocket_client::WebsocketError;

#[derive(Debug)]
pub enum Error {
    ServiceShutdownUnexpectedly,

    UnableToCreateOutputFile,
    UnableToLoadOutputFile,
    UnableToSaveOutputFile,

    UnableToDecode,
    UnableToDeserialize,
    UnableToFetchAccount,

    InvalidOraclePriceData,

    TransactionError,

    RpcError,
    WebsocketClientError(WebsocketError),
}

impl From<ClientError> for Error {
    fn from(_: ClientError) -> Self {
        Self::RpcError
    }
}

impl From<WebsocketError> for Error {
    fn from(value: WebsocketError) -> Self {
        Self::WebsocketClientError(value)
    }
}

impl ToString for Error {
    fn to_string(&self) -> String {
        match self {
            Self::ServiceShutdownUnexpectedly => "ServiceShutdownUnexpectedly".to_string(),
            Self::UnableToCreateOutputFile => "UnableToCreateOutputFile".to_string(),
            Self::UnableToLoadOutputFile => "UnableToLoadOutputFile".to_string(),
            Self::UnableToSaveOutputFile => "UnableToSaveOutputFile".to_string(),
            Self::UnableToDecode => "UnableToDecode".to_string(),
            Self::UnableToDeserialize => "UnableToDeserialize".to_string(),
            Self::UnableToFetchAccount => "UnableToFetchAccount".to_string(),
            Self::InvalidOraclePriceData => "InvalidOraclePriceData".to_string(),
            Self::TransactionError => "TransactionError".to_string(),
            Self::RpcError => "RpcError".to_string(),
            Self::WebsocketClientError(e) => format!("WebsocketClientError: {}", e.to_string()),
        }
    }
}
