use std::collections::HashMap;

use bot::{
    args::{self, CliArgs, Commands, Wallet},
    error::Error,
    utils::websocket_client::WebsocketClient,
};
use clap::Parser;
use funding_program::{client::state::load_funding_account, state::Exchange};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::{
    commitment_config::CommitmentConfig, pubkey::Pubkey, signature::Keypair, signer::Signer,
};
use tokio::{
    fs::{self, remove_file, File},
    io::AsyncWriteExt,
};

#[tokio::main]
async fn main() -> Result<(), Error> {
    let cli_args = CliArgs::parse();
    dotenv::dotenv().ok();

    let rpc_client = args::load_and_parse("RPC_URL", |url| {
        Ok(RpcClient::new_with_commitment(
            url,
            CommitmentConfig::confirmed(),
        ))
    });
    let ws_client = args::load_and_parse("WS_URL", |url| Ok(WebsocketClient::new(url)));
    let wallet = args::load_and_parse("PRIVATE_KEY", |pk_str| {
        let bytes = pk_str
            .split(",")
            .map(|b| b.parse().map_err(|_| "Invalid private key"))
            .collect::<Result<Vec<u8>, &str>>()?;
        let keypair = Keypair::from_bytes(&bytes).map_err(|_| "Invalid private key")?;
        let pubkey = keypair.try_pubkey().unwrap();
        Ok(Wallet { keypair, pubkey })
    });
    let (mango_markets, drift_markets) = args::load_and_parse("MARKETS", |markets| {
        let markets = markets.split(",").map(|x| x.to_string()).collect();
        Ok((
            args::parse_mango_markets_into_ids(&markets).map_err(|e| e.to_string())?,
            args::parse_drift_markets_into_ids(&markets).map_err(|e| e.to_string())?,
        ))
    });

    match cli_args.commands {
        // outputs funding accounts to a file
        //
        // authority: #authority
        // ------------------------------------
        // #address : #id #market_index #exchange (#exchange_discriminant)
        Commands::FindFundingAccounts { output_dir } => {
            let filename = "funding_accounts.txt";
            let mut file_path = output_dir;
            file_path.push(filename);

            let mut out_file = {
                if let Ok(_) = fs::metadata(&file_path).await {
                    remove_file(&file_path).await.map_err(|e| {
                        println!("File could not be loaded: {}", e.to_string());
                        Error::UnableToLoadOutputFile
                    })?;
                }

                File::create(&file_path).await.map_err(|e| {
                    println!("File could not be created: {}", e.to_string());
                    Error::UnableToCreateOutputFile
                })?
            };

            let funding_accounts = rpc_client
                .get_program_accounts(&funding_program::id())
                .await?;
            let mut accounts_by_authority: HashMap<Pubkey, String> = HashMap::new();

            for (address, ai) in funding_accounts.iter() {
                let account = load_funding_account(&ai.data).map_err(|_| {
                    println!("Invalid funding account");
                    Error::UnableToDeserialize
                })?;

                let exchange = match account.exchange {
                    Exchange::Drift => "drift",
                    Exchange::Mango => "mango",
                };
                let meta = format!(
                    "{}: {} {} {}\n",
                    address.to_string(),
                    account.id,
                    account.market_index,
                    exchange
                );
                match accounts_by_authority.get_mut(&account.authority) {
                    Some(accounts) => {
                        accounts.push_str(&meta);
                    }
                    None => {
                        accounts_by_authority.insert(account.authority, meta);
                    }
                }
            }

            let mut output =
                "<account_address>: <id> <market_index> <exchange> (<exchange_discriminant>)\n"
                    .to_string();
            accounts_by_authority.iter().for_each(|(authority, meta)| {
                output.push_str(&format!("\nAuthority: {}\n", authority.to_string()));
                output.push_str("--------------------------------\n");
                output.push_str(&format!("{meta}\n"));
            });

            match out_file.write_all(output.as_bytes()).await {
                Ok(_) => {
                    out_file.flush().await.map_err(|e| {
                        println!("Could not save funding accounts: {}", e.to_string());
                        Error::UnableToSaveOutputFile
                    })?;

                    println!("Found {} funding accounts", funding_accounts.len());
                }
                Err(e) => {
                    println!("Could not save funding accounts: {}", e.to_string());
                    return Err(Error::UnableToSaveOutputFile);
                }
            }
        }
        Commands::FundingClient => {}
        Commands::Bot => {}
    }

    Ok(())
}
