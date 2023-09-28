use solana_client::client_error::ClientError;

use crate::{
    args::ParseMarketsError,
    utils::{transaction::TransactionErrorClient, websocket_client::WebsocketError},
};

#[derive(Debug)]
pub enum Error {
    DriftOrderbookIsEmpty,
    UnableToGetDriftOrderPrice,
    UnableToGetDriftAmmPrice,

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
    TransactionErrorClient(TransactionErrorClient),
    ParseMarketsError(ParseMarketsError),
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

impl From<TransactionErrorClient> for Error {
    fn from(value: TransactionErrorClient) -> Self {
        Self::TransactionErrorClient(value)
    }
}

impl From<ParseMarketsError> for Error {
    fn from(value: ParseMarketsError) -> Self {
        Self::ParseMarketsError(value)
    }
}

impl ToString for Error {
    fn to_string(&self) -> String {
        match self {
            Self::DriftOrderbookIsEmpty => "DriftOrderbookIsEmpty".to_string(),
            Self::UnableToGetDriftOrderPrice => "UnableToGetDriftOrderPrice".to_string(),
            Self::UnableToGetDriftAmmPrice => "UnableToGetDriftAmmReservePrice".to_string(),
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
            Self::TransactionErrorClient(e) => format!("TransactionErrorClient: {}", e.to_string()),
            Self::ParseMarketsError(e) => format!("ParseMarketsError: {}", e.to_string()),
        }
    }
}
