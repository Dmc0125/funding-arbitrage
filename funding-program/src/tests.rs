use std::{rc::Rc, sync::Arc, time::Duration};

use bot::{
    error::Error,
    utils::transaction::{self},
};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_program::{native_token::LAMPORTS_PER_SOL, pubkey::Pubkey};
use solana_sdk::{
    commitment_config::CommitmentConfig, signature::Keypair, signer::Signer,
    transaction::Transaction,
};
use tokio::time::sleep;

use crate::{
    client::{
        instructions::{self, InitializeFundingAccountAccounts},
        state::load_funding_account,
    },
    state::{Exchange, FundingAccountLoader},
};

const RPC_URL: &'static str = "http://127.0.0.1:8899";

struct Wallet {
    keypair: Keypair,
    pubkey: Pubkey,
}

async fn mock_wallet<'a>(rpc_client: &Arc<RpcClient>) -> Rc<Wallet> {
    let keypair = Keypair::new();
    let pubkey = keypair.try_pubkey().unwrap();
    println!("Mock address: {}", pubkey);

    let lamports: u64 = 1000 * LAMPORTS_PER_SOL;
    let sig = rpc_client.request_airdrop(&pubkey, lamports).await;
    assert!(sig.is_ok());

    sleep(Duration::from_secs(5)).await;
    let res = rpc_client
        .get_signature_status_with_commitment(&sig.unwrap(), CommitmentConfig::confirmed())
        .await;
    assert!(res.is_ok());
    let res = res.unwrap();
    assert!(res.is_some());
    assert!(res.unwrap().is_ok());

    Rc::new(Wallet { keypair, pubkey })
}

async fn initialize_success(
    rpc_client: &Arc<RpcClient>,
    wallet: &Rc<Wallet>,
) -> Result<(Pubkey, Pubkey), Error> {
    let drift_funding_account = FundingAccountLoader::pda(0, 0, &Exchange::Drift).0;
    let mango_funding_account = FundingAccountLoader::pda(0, 0, &Exchange::Mango).0;
    let blockhash = rpc_client.get_latest_blockhash().await?;

    let ixs = [
        instructions::initialize_funding_account(
            InitializeFundingAccountAccounts {
                authority: wallet.pubkey,
                funding_account: drift_funding_account,
            },
            0,
            Exchange::Drift,
            0,
            300,
            600,
            5,
            12,
        ),
        instructions::initialize_funding_account(
            InitializeFundingAccountAccounts {
                authority: wallet.pubkey,
                funding_account: mango_funding_account,
            },
            0,
            Exchange::Mango,
            0,
            300,
            600,
            5,
            12,
        ),
    ];

    let tx = Transaction::new_signed_with_payer(
        &ixs,
        Some(&wallet.pubkey),
        &[&wallet.keypair],
        blockhash,
    );

    let res = transaction::send_and_confirm_transaction(rpc_client, &tx).await?;
    assert!(res.is_success());

    let funding_ais = rpc_client
        .get_multiple_accounts(&[drift_funding_account, mango_funding_account])
        .await?;

    for (i, exchange) in [Exchange::Drift, Exchange::Mango].iter().enumerate() {
        let ai = funding_ais[i].clone();
        assert!(ai.is_some());
        let ai = ai.unwrap();
        let funding_account = load_funding_account(&ai.data);
        assert!(funding_account.is_ok());
        let funding_account = funding_account.unwrap();

        assert_eq!(&funding_account.exchange, exchange);
        assert_eq!(funding_account.authority, wallet.pubkey);
        assert_eq!(funding_account.market_index, 0);
        assert_eq!(funding_account.funding_ema, None);
        assert_eq!(funding_account.id, 0);
        assert_eq!(funding_account.last_updated_ts, 0);
        assert_eq!(funding_account.config.period_length, 5);
        assert_eq!(funding_account.config.update_frequency_secs, 300);
        assert_eq!(funding_account.config.staleness_threshold_secs, 600);
        assert_eq!(funding_account.config.data_points_count, 12);
        assert_eq!(ai.data.len(), FundingAccountLoader::size(12));

        assert!(funding_account.data_points.iter().all(|x| x.is_none()))
    }

    Ok((drift_funding_account, mango_funding_account))
}

async fn configure_funding_account(
    rpc_client: &Arc<RpcClient>,
    wallet: &Rc<Wallet>,
    drift_address: Pubkey,
) -> Result<(), Error> {
    let blockhash = rpc_client.get_latest_blockhash().await?;
    let ixs = [instructions::configure_funding_account(
        instructions::ConfigureFundingAccountAccounts {
            authority: wallet.pubkey,
            funding_account: drift_address,
        },
        Some(1000),
        Some(2000),
        None,
        None,
    )];

    let tx = Transaction::new_signed_with_payer(
        &ixs,
        Some(&wallet.pubkey),
        &[&wallet.keypair],
        blockhash,
    );

    let res = transaction::send_and_confirm_transaction(rpc_client, &tx).await?;
    assert!(res.is_success());

    let ai = rpc_client.get_account(&drift_address).await?;
    let account = load_funding_account(&ai.data).unwrap();

    assert_eq!(account.config.update_frequency_secs, 1000);
    assert_eq!(account.config.staleness_threshold_secs, 2000);
    assert_eq!(account.config.period_length, 5);

    Ok(())
}

async fn increase_data_points_count(
    rpc_client: &Arc<RpcClient>,
    wallet: &Rc<Wallet>,
    drift_address: Pubkey,
) -> Result<(), Error> {
    let blockhash = rpc_client.get_latest_blockhash().await?;
    let ixs = [instructions::configure_funding_account(
        instructions::ConfigureFundingAccountAccounts {
            authority: wallet.pubkey,
            funding_account: drift_address,
        },
        None,
        None,
        None,
        Some(20),
    )];

    let tx = Transaction::new_signed_with_payer(
        &ixs,
        Some(&wallet.pubkey),
        &[&wallet.keypair],
        blockhash,
    );

    let res = transaction::send_and_confirm_transaction(rpc_client, &tx).await?;
    assert!(res.is_success());

    let ai = rpc_client.get_account(&drift_address).await?;
    let account = load_funding_account(&ai.data).unwrap();

    assert_eq!(ai.data.len(), FundingAccountLoader::size(20));
    assert_eq!(account.data_points[0], Some(10_0000_i64));
    assert!(account.data_points[1..].iter().all(|x| x.is_none()));

    Ok(())
}

async fn decrease_data_points_count(
    rpc_client: &Arc<RpcClient>,
    wallet: &Rc<Wallet>,
    drift_address: Pubkey,
) -> Result<(), Error> {
    let blockhash = rpc_client.get_latest_blockhash().await?;
    let ixs = [instructions::configure_funding_account(
        instructions::ConfigureFundingAccountAccounts {
            authority: wallet.pubkey,
            funding_account: drift_address,
        },
        None,
        None,
        None,
        Some(10),
    )];

    let tx = Transaction::new_signed_with_payer(
        &ixs,
        Some(&wallet.pubkey),
        &[&wallet.keypair],
        blockhash,
    );

    let res = transaction::send_and_confirm_transaction(rpc_client, &tx).await?;
    assert!(res.is_success());

    let ai = rpc_client.get_account(&drift_address).await?;
    let account = load_funding_account(&ai.data).unwrap();

    assert_eq!(ai.data.len(), FundingAccountLoader::size(10));
    dbg!(&account.data_points);
    assert!(account.data_points.iter().all(|x| x.is_none()));

    Ok(())
}

async fn update_err_wrong_authority(
    rpc_client: &Arc<RpcClient>,
    drift_address: Pubkey,
) -> Result<(), Error> {
    let fake_wallet = mock_wallet(rpc_client).await;

    let blockhash = rpc_client.get_latest_blockhash().await?;

    let ixs = [instructions::update_funding_accounts(
        instructions::UpdateFundingAccountAccounts {
            authority: fake_wallet.pubkey,
            funding_account: drift_address,
        },
        10_0000,
    )];
    let tx = Transaction::new_signed_with_payer(
        &ixs,
        Some(&fake_wallet.pubkey),
        &[&fake_wallet.keypair],
        blockhash,
    );

    let res = transaction::send_and_confirm_transaction(rpc_client, &tx).await?;
    assert!(res.is_err());

    Ok(())
}

async fn update_success(
    rpc_client: &Arc<RpcClient>,
    wallet: &Rc<Wallet>,
    drift_address: Pubkey,
    mango_address: Pubkey,
) -> Result<(), Error> {
    let blockhash = rpc_client.get_latest_blockhash().await?;

    let ixs = [
        instructions::update_funding_accounts(
            instructions::UpdateFundingAccountAccounts {
                authority: wallet.pubkey,
                funding_account: drift_address,
            },
            10_0000,
        ),
        instructions::update_funding_accounts(
            instructions::UpdateFundingAccountAccounts {
                authority: wallet.pubkey,
                funding_account: mango_address,
            },
            0,
        ),
    ];
    let tx = Transaction::new_signed_with_payer(
        &ixs,
        Some(&wallet.pubkey),
        &[&wallet.keypair],
        blockhash,
    );

    let res = transaction::send_and_confirm_transaction(rpc_client, &tx).await?;
    assert!(res.is_success());

    let ai = rpc_client.get_account(&drift_address).await?;
    let account = load_funding_account(&ai.data).unwrap();

    assert_eq!(account.data_points[0], Some(10_0000_i64));
    assert_eq!(account.funding_ema, None);
    assert_ne!(account.last_updated_ts, 0);

    Ok(())
}

async fn update_err_too_soon(
    rpc_client: &Arc<RpcClient>,
    wallet: &Rc<Wallet>,
    drift_address: Pubkey,
) -> Result<(), Error> {
    let blockhash = rpc_client.get_latest_blockhash().await?;

    let ixs = [instructions::update_funding_accounts(
        instructions::UpdateFundingAccountAccounts {
            authority: wallet.pubkey,
            funding_account: drift_address,
        },
        10_0000,
    )];
    let tx = Transaction::new_signed_with_payer(
        &ixs,
        Some(&wallet.pubkey),
        &[&wallet.keypair],
        blockhash,
    );

    let res = transaction::send_and_confirm_transaction(rpc_client, &tx).await?;
    assert!(res.is_err());

    Ok(())
}

async fn close_account_success(
    rpc_client: &Arc<RpcClient>,
    wallet: &Rc<Wallet>,
    drift_address: Pubkey,
) -> Result<(), Error> {
    let blockhash = rpc_client.get_latest_blockhash().await?;
    let ixs = [instructions::close_funding_account(
        instructions::CloseFundingAccountAccounts {
            authority: wallet.pubkey,
            funding_account: drift_address,
            receiver: wallet.pubkey,
        },
    )];

    let tx = Transaction::new_signed_with_payer(
        &ixs,
        Some(&wallet.pubkey),
        &[&wallet.keypair],
        blockhash,
    );

    let res = transaction::send_and_confirm_transaction(rpc_client, &tx).await?;
    assert!(res.is_success());

    Ok(())
}

#[tokio::test]
async fn test() {
    let rpc_client = Arc::new(RpcClient::new_with_commitment(
        RPC_URL.to_string(),
        CommitmentConfig::confirmed(),
    ));
    let wallet = mock_wallet(&rpc_client).await;

    let init_res = initialize_success(&rpc_client, &wallet).await;
    assert!(init_res.is_ok());
    let (drift_address, mango_address) = init_res.unwrap();

    assert!(update_err_wrong_authority(&rpc_client, drift_address)
        .await
        .is_ok());

    assert!(
        configure_funding_account(&rpc_client, &wallet, drift_address)
            .await
            .is_ok()
    );

    assert!(
        update_success(&rpc_client, &wallet, drift_address, mango_address)
            .await
            .is_ok()
    );

    assert!(update_err_too_soon(&rpc_client, &wallet, drift_address)
        .await
        .is_ok());

    assert!(
        increase_data_points_count(&rpc_client, &wallet, drift_address)
            .await
            .is_ok()
    );

    assert!(
        decrease_data_points_count(&rpc_client, &wallet, drift_address)
            .await
            .is_ok()
    );

    assert!(close_account_success(&rpc_client, &wallet, drift_address)
        .await
        .is_ok());
}
