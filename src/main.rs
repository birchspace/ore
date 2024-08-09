mod args;
mod claim;
mod constant;
mod cu_limits;
mod jito;

mod send_and_confirm;

mod utils;

use std::sync::Arc;

use args::*;
use clap::{command, Parser, Subcommand};

use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::{commitment_config::CommitmentConfig, signature::Keypair};

struct Miner {
    pub private_key: Option<String>,
    pub priority_fee: u64,
    pub rpc_client: Arc<RpcClient>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    #[command(about = "Claim your mining rewards")]
    Claim(ClaimArgs),
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
    let rpc_client = RpcClient::new_with_commitment(cluster, CommitmentConfig::confirmed());

    let miner = Arc::new(Miner::new(Arc::new(rpc_client), args.priority_fee, None));

    match args.command {
        Commands::Claim(args) => {
            miner.claim_from_keys(args).await;
        }
    }
}

impl Miner {
    pub fn new(rpc_client: Arc<RpcClient>, priority_fee: u64, private_key: Option<String>) -> Self {
        Self {
            rpc_client,
            private_key,
            priority_fee,
        }
    }

    pub fn signer(&self) -> Keypair {
        match self.private_key.clone() {
            Some(key) => Keypair::from_base58_string(&key),
            None => panic!("No Private Key provided"),
        }
    }
}
