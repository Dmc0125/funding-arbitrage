use solana_sdk::{pubkey::Pubkey, signature::Keypair};

pub struct Wallet {
    pub keypair: Keypair,
    pub pubkey: Pubkey,
}
