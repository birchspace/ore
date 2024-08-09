mod args;
mod claim;
mod constant;
mod cu_limits;
#[cfg(feature = "admin")]
mod initialize;
mod jito;
mod mine;
mod open;

mod send_and_confirm;

mod utils;

use std::sync::Arc;

use args::*;
use clap::{command, Parser, Subcommand};
use jito::{subscribe_jito_tips, JitoTips};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::{commitment_config::CommitmentConfig, signature::Keypair};
use tokio::sync::RwLock;

struct Miner {
    pub private_key: Option<String>,
    pub priority_fee: u64,
    pub rpc_client: Arc<RpcClient>,
    tips: Arc<RwLock<JitoTips>>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    #[command(about = "Claim your mining rewards")]
    Claim(ClaimArgs),

    #[command(about = "Start mining")]
    Mine(MineArgs),
}

#[derive(Parser, Debug)]
#[command(about, version)]
struct Args {
    #[arg(
        long,
        value_name = "NETWORK_URL",
        help = "Network address of your RPC provider",
        global = true
    )]
    rpc: Option<String>,

    #[clap(
        global = true,
        short = 'C',
        long = "config",
        id = "PATH",
        help = "Filepath to config file."
    )]
    config_file: Option<String>,

    #[arg(
        long,
        value_name = "private_key",
        help = "Private Key to keypair to use",
        global = true
    )]
    keypair: Option<String>,

    #[arg(
        long,
        value_name = "MICROLAMPORTS",
        help = "Number of microlamports to pay as priority fee per transaction",
        default_value = "0",
        global = true
    )]
    priority_fee: u64,

    #[command(subcommand)]
    command: Commands,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    // Load the config file from custom path, the default path, or use default config values
    let cli_config = if let Some(config_file) = &args.config_file {
        solana_cli_config::Config::load(config_file).unwrap_or_else(|_| {
            eprintln!("error: Could not find config file `{}`", config_file);
            std::process::exit(1);
        })
    } else if let Some(config_file) = &*solana_cli_config::CONFIG_FILE {
        solana_cli_config::Config::load(config_file).unwrap_or_default()
    } else {
        solana_cli_config::Config::default()
    };

    // Initialize miner.
    let cluster = args.rpc.unwrap_or(cli_config.json_rpc_url);
    let default_keypair = args.keypair.unwrap_or(cli_config.keypair_path);
    let rpc_client = RpcClient::new_with_commitment(cluster, CommitmentConfig::confirmed());

    // jito
    let tips = Arc::new(RwLock::new(JitoTips::default()));
    // subscribe_jito_tips(tips.clone()).await;

    let miner = Arc::new(Miner::new(
        Arc::new(rpc_client),
        args.priority_fee,
        Some(default_keypair),
        tips,
    ));

    // Execute user command.
    match args.command {
        Commands::Claim(args) => {
            miner.claim(args).await;
        }

        Commands::Mine(args) => {
            miner.mine(args).await;
        }
    }
}

impl Miner {
    pub fn new(
        rpc_client: Arc<RpcClient>,
        priority_fee: u64,
        private_key: Option<String>,
        tips: Arc<RwLock<JitoTips>>,
    ) -> Self {
        Self {
            rpc_client,
            private_key,
            priority_fee,
            tips,
        }
    }

    pub fn signer(&self) -> Keypair {
        match self.private_key.clone() {
            Some(key) => Keypair::from_base58_string(&key),
            None => panic!("No Private Key provided"),
        }
    }
}
