use clap::{arg, Parser};

#[derive(Parser, Debug)]
pub struct ClaimArgs {
    #[arg(
        long,
        value_name = "AMOUNT",
        help = "The amount of rewards to claim. Defaults to max."
    )]
    pub amount: Option<f64>,

    #[arg(
        long,
        value_name = "WALLET_ADDRESS",
        help = "Wallet to receive claimed tokens."
    )]
    pub to: String,

    #[arg(long, help = "Path to keys.txt file containing private keys")]
    pub keys_file: String,

    #[arg(long, help = "Auto-claim threshold amount")]
    pub autoclaimnum: Option<f64>,

    #[arg(long, help = "check balance from secs")]
    pub check_time: u64,
}
