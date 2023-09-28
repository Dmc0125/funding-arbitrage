use std::path::PathBuf;

use clap::{Parser, Subcommand};
use solana_sdk::{pubkey::Pubkey, signature::Keypair};

pub struct Wallet {
    pub keypair: Keypair,
    pub pubkey: Pubkey,
}

const NAMESPACE: &'static str = "[DOTENV_ERROR]:";

pub fn load_arg(key: &str) -> String {
    std::env::var(key).expect(&format!("{NAMESPACE} Argument \"{key}\" is missing"))
}

pub fn load_and_parse<T, F: Fn(String) -> Result<T, String>>(key: &str, parse_fn: F) -> T {
    match parse_fn(load_arg(key)) {
        Err(msg) => {
            panic!("{NAMESPACE} Argument \"{key}\" could not be parsed: {msg}");
        }
        Ok(val) => val,
    }
}

#[derive(Debug)]
pub struct ParseMarketsError(pub String);

impl ParseMarketsError {
    pub fn to_string(&self) -> String {
        format!("Unable to parse markets: {}", self.0)
    }
}

pub fn parse_mango_markets_into_ids(markets: &Vec<String>) -> Result<Vec<u16>, ParseMarketsError> {
    markets
        .iter()
        .map(|market| {
            let id = match market.as_str() {
                "BTC" => 0,
                "SOL" => 2,
                "ETH" => 3,
                "RNDR" => 4,
                _ => {
                    return Err(ParseMarketsError("mango".to_string()));
                }
            };
            Ok(id)
        })
        .collect()
}

pub fn parse_drift_markets_into_ids(markets: &Vec<String>) -> Result<Vec<u16>, ParseMarketsError> {
    markets
        .iter()
        .map(|market| {
            let id = match market.as_str() {
                "SOL" => 0,
                "BTC" => 1,
                "ETH" => 2,
                "RNDR" => 12,
                _ => {
                    return Err(ParseMarketsError("drift".to_string()));
                }
            };
            Ok(id)
        })
        .collect()
}

#[derive(Debug, Parser)]
pub struct CliArgs {
    #[command(subcommand)]
    pub commands: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    FindFundingAccounts {
        #[arg(long)]
        output_dir: PathBuf,
    },

    FundingClient {
        markets: Vec<String>,
    },

    Bot {
        markets: Vec<String>,
    },
}
