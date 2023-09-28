use std::{collections::HashMap, sync::Arc};

use bot::{
    addresses::StaticAddresses,
    args::{self, CliArgs, Commands, Wallet},
    error::Error,
    services::funding_relayer::{initialize_funding_accounts_if_needed, start_funding_relayer},
    state::{fetch_markets, State},
    utils::websocket_client::{create_persisted_websocket_connection, WebsocketClient},
};
use clap::Parser;
use drift::accounts::PerpMarket as DriftPerpMarket;
use funding_program::{client::state::load_funding_account, state::Exchange};
use mango::accounts::PerpMarket as MangoPerpMarket;
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
        Ok(Arc::new(RpcClient::new_with_commitment(
            url,
            CommitmentConfig::confirmed(),
        )))
    });
    let ws_client = args::load_and_parse("WS_URL", |url| Ok(Arc::new(WebsocketClient::new(url))));
    let wallet = args::load_and_parse("PRIVATE_KEY", |pk_str| {
        let bytes = pk_str
            .split(",")
            .map(|b| b.parse().map_err(|_| "Invalid private key"))
            .collect::<Result<Vec<u8>, &str>>()?;
        let keypair = Keypair::from_bytes(&bytes).map_err(|_| "Invalid private key")?;
        let pubkey = keypair.try_pubkey().unwrap();
        Ok(Arc::new(Wallet { keypair, pubkey }))
    });

    start(cli_args, rpc_client, ws_client, wallet).await
}

async fn start(
    cli_args: CliArgs,
    rpc_client: Arc<RpcClient>,
    _ws_client: Arc<WebsocketClient>,
    wallet: Arc<Wallet>,
) -> Result<(), Error> {
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
                    Exchange::Drift => "drift (0)",
                    Exchange::Mango => "mango (1)",
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
        Commands::FundingClient { markets } => {
            let mango_markets_addresses = StaticAddresses::get_mango_markets_from_ids(
                &args::parse_mango_markets_into_ids(&markets)?,
            );
            let drift_markets_addresses = StaticAddresses::get_drift_markets_from_ids(
                &args::parse_drift_markets_into_ids(&markets)?,
            );

            let mut static_addresses = StaticAddresses::new();

            let mango_markets =
                fetch_markets::<MangoPerpMarket>(&rpc_client, &mango_markets_addresses).await?;
            let drift_markets =
                fetch_markets::<DriftPerpMarket>(&rpc_client, &drift_markets_addresses).await?;

            static_addresses.set_mango_markets(&mango_markets);
            static_addresses.set_drift_markets(&drift_markets);

            initialize_funding_accounts_if_needed(
                &rpc_client,
                &wallet,
                &static_addresses.funding_accounts,
            )
            .await?;

            let state = State::new(rpc_client.clone(), static_addresses);

            *state.mango_markets.write().await = mango_markets;
            *state.drift_markets.write().await = drift_markets;

            let (relayer_cache_handle, relayer_handle) =
                start_funding_relayer(rpc_client, wallet, Arc::new(state)).await?;

            let program_result = tokio::select! {
                relayer_cache_res = relayer_cache_handle => {
                    relayer_cache_res
                }
                relayer_res = relayer_handle => {
                    relayer_res
                }
            };

            match program_result {
                Ok(res) => {
                    res?;
                }
                Err(e) => {
                    dbg!(e);
                }
            }

            return Err(Error::ServiceShutdownUnexpectedly);
        }
        Commands::Bot { markets } => {
            // let websocket_handle = create_persisted_websocket_connection(ws_client.clone()).await?;

            // let mango_markets_ids = args::parse_mango_markets_into_ids(&markets)?;
            // let drift_markets_ids = args::parse_drift_markets_into_ids(&markets)?;

            // let mango_markets_addresses =
            //     StaticAddresses::get_mango_markets_from_ids(&mango_markets_ids);
            // let drift_markets_addresses =
            //     StaticAddresses::get_drift_markets_from_ids(&drift_markets_ids);

            // let mango_markets =
            //     fetch_markets::<MangoPerpMarket>(&rpc_client, mango_markets_addresses).await?;
            // let drift_markets =
            //     fetch_markets::<DriftPerpMarket>(&rpc_client, drift_markets_addresses).await?;

            // let mut static_addresses = StaticAddresses::new();
            // static_addresses.set_mango_markets(&mango_markets);
            // static_addresses.set_drift_markets(&drift_markets);

            // let (mut state, state_update_sender, state_update_receiver) = State::new();

            // let funding_accounts = fetch_funding_accounts(&rpc_client, &static_addresses).await?;
            // state.set_initial_funding_accounts(funding_accounts).await;

            // let funding_accounts_subscription_handle = subscribe_to_funding_accounts(
            //     ws_client.clone(),
            //     &static_addresses,
            //     state_update_sender.clone(),
            // );
            // let oracles_subscription_handle =
            //     subscribe_to_oracles(ws_client.clone(), &static_addresses, state_update_sender);

            // let state = Arc::new(state);
            // let state_handle =
            //     State::subscribe_to_state_updates(state.clone(), state_update_receiver);

            // let bot_handle = start_bot(
            //     rpc_client,
            //     ws_client,
            //     state,
            //     drift_markets_ids,
            //     mango_markets_ids,
            // );

            // let program_result = tokio::select! {
            //     websocket_res = websocket_handle => {
            //         websocket_res.map(|r| r.map_err(|e| e.into()))
            //     }
            //     (oracles_res, _, _) = oracles_subscription_handle => {
            //         oracles_res
            //     }
            //     funding_accounts_res = funding_accounts_subscription_handle => {
            //         funding_accounts_res
            //     }
            //     _ = state_handle => {
            //         println!("State subscription shutdown unexpectedly");
            //         Ok(Ok(()))
            //     }
            // };

            // match program_result {
            //     Ok(res) => {
            //         res?;
            //     }
            //     Err(e) => {
            //         dbg!(e);
            //     }
            // }

            // return Err(Error::ServiceShutdownUnexpectedly);
        }
    }

    Ok(())
}
