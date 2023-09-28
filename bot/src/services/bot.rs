// use std::{collections::HashMap, str::FromStr, sync::Arc, time::Duration};

// use anchor_lang::Discriminator;
// use funding_program::state::Exchange;
// use solana_client::nonblocking::rpc_client::RpcClient;
// use solana_sdk::pubkey::Pubkey;
// use tokio::{
//     sync::{mpsc, Mutex},
//     task::JoinHandle,
//     time::sleep,
// };
// use tokio_stream::StreamExt;

// use crate::{
//     error::Error,
//     state::{get_program_subscribe_config, start_drift_orderbooks_process, State},
//     utils::{deser::AccountData, websocket_client::WebsocketClient},
// };

// pub fn open_position(rpc_client: &Arc<RpcClient>) {}

// const OPEN_DIFF_THRESHOLD_PCT_NO_DECIMALS: i64 = 20;
// const OPEN_DIFF_THRESHOLD_PCT: i64 = OPEN_DIFF_THRESHOLD_PCT_NO_DECIMALS * 1_000_000;

// const CLOSE_DIFF_THRESHOLD_PCT_NO_DECIMALS: i64 = 10;
// const CLOSE_DIFF_THRESHOLD_PCT: i64 = OPEN_DIFF_THRESHOLD_PCT_NO_DECIMALS * 1_000_000;

// async fn find_highest_funding_rates_diff(
//     state: &Arc<State>,
//     drift_markets_ids: Vec<u16>,
//     mango_markets_ids: Vec<u16>,
// ) -> Option<(u16, u16, i64)> {
//     let funding_accounts = &state.funding_accounts.lock().await;
//     let mut highest_diff: Option<(u16, u16, i64)> = None;

//     for (drift_market_id, mango_market_id) in drift_markets_ids.iter().zip(mango_markets_ids.iter())
//     {
//         let drift_funding_account = funding_accounts
//             .iter()
//             .find(|fa| &fa.market_index == drift_market_id && fa.exchange == Exchange::Drift);
//         let mango_funding_account = funding_accounts
//             .iter()
//             .find(|fa| &fa.market_index == mango_market_id && fa.exchange == Exchange::Mango);

//         let (Some(drift_funding_account), Some(mango_funding_account)) =
//             (drift_funding_account, mango_funding_account)
//         else {
//             println!(
//                 "Missing funding accounts - drift: {} - mango: {}",
//                 drift_market_id, mango_market_id
//             );
//             continue;
//         };

//         let diff = match (
//             drift_funding_account.funding_ema,
//             mango_funding_account.funding_ema,
//         ) {
//             (Some(drift_ema), Some(mango_ema)) => {
//                 println!(
//                     "drift {} - mango {}",
//                     drift_funding_account.market_index, mango_funding_account.market_index
//                 );
//                 println!("Drift {} - Mango {}", drift_ema, mango_ema);
//                 drift_ema - mango_ema
//             }
//             _ => {
//                 println!(
//                     "Funding accounts stale - drift ema {:?} - mango ema {:?}",
//                     drift_funding_account.funding_ema, mango_funding_account.funding_ema
//                 );
//                 continue;
//             }
//         };

//         if diff > OPEN_DIFF_THRESHOLD_PCT
//             && highest_diff
//                 .map(|(_, _, highest_diff)| highest_diff > diff)
//                 .unwrap_or(true)
//         {
//             highest_diff = Some((*drift_market_id, *mango_market_id, diff));
//         }
//     }

//     highest_diff
// }

// pub fn start_bot(
//     rpc_client: Arc<RpcClient>,
//     ws_client: Arc<WebsocketClient>,
//     state: Arc<State>,
//     drift_markets_ids: Vec<u16>,
//     mango_markets_ids: Vec<u16>,
// ) -> JoinHandle<Result<(), Error>> {
//     let (drift_orderbooks, drift_orderbooks_handle) =
//         start_drift_orderbooks_process(rpc_client, ws_client, &drift_markets_ids);

//     tokio::spawn(async move {
//         loop {
//             if let Some((drift_market_index, mango_market_index, diff)) =
//                 find_highest_funding_rates_diff(
//                     &state,
//                     drift_markets_ids.clone(),
//                     mango_markets_ids.clone(),
//                 )
//                 .await
//             {
//                 drift_orderbooks.subscribe().await;
//             } else {
//                 println!("Arbitrage opportunity does not exist")
//             }

//             sleep(Duration::from_secs(300)).await;
//         }
//     })
// }
