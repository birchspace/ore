use colored::*;
use drillx::Solution;
use futures_util::{SinkExt, StreamExt};
use ore_api::{
    consts::{BUS_ADDRESSES, BUS_COUNT, EPOCH_DURATION},
    state::{Config, Proof},
};
use rand::Rng;
use solana_program::pubkey::Pubkey;
use solana_sdk::signer::Signer;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Result;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::{
    args::MineArgs,
    send_and_confirm::ComputeBudget,
    utils::{amount_u64_to_string, get_clock, get_config, get_proof_with_authority, proof_pubkey},
    Miner,
};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
struct ClientSolution {
    nonce: u64,
    difficulty: u32,
    hash: HashWrapper,
}

#[derive(Serialize, Deserialize, Debug)]
struct HashWrapper {
    h: Vec<u8>,
    d: [u8; 16],
}

impl Miner {
    pub async fn mine(&self, args: MineArgs) {
        // Register, if needed.
        let signer = self.signer();
        self.open().await;

        // Check num threads
        self.check_num_cores(args.threads);

        // Start mining loop
        loop {
            // Fetch proof
            let proof = get_proof_with_authority(&self.rpc_client, signer.pubkey()).await;

            println!(
                "\nStake balance: {} ORE",
                amount_u64_to_string(proof.balance)
            );

            // Calc cutoff time
            let cutoff_time = self.get_cutoff(proof, args.buffer_time).await;

            // Run drillx
            let config = get_config(&self.rpc_client).await;
            let solution = Self::find_hash_par(
                proof,
                cutoff_time,
                args.threads,
                config.min_difficulty as u32,
                args.ip,
                args.port,
            )
            .await;

            // Submit most difficult hash
            let mut compute_budget = 500_000;
            let mut ixs = vec![ore_api::instruction::auth(proof_pubkey(signer.pubkey()))];
            if self.should_reset(config).await && rand::thread_rng().gen_range(0..100).eq(&0) {
                compute_budget += 100_000;
                ixs.push(ore_api::instruction::reset(signer.pubkey()));
            }
            ixs.push(ore_api::instruction::mine(
                signer.pubkey(),
                signer.pubkey(),
                find_bus(),
                solution,
            ));
            self.send_and_confirm(&ixs, ComputeBudget::Fixed(compute_budget), false)
                .await
                .ok();
        }
    }

    async fn find_hash_par(
        proof: Proof,
        _cutoff_time: u64,
        _threads: u64,
        _min_difficulty: u32,
        ip: u64,
        port: u64,
    ) -> Solution {
        let url = format!("{ip}:{port}");
        let mut best_hash = String::from("value");
        let (mut ws_stream, _) = connect_async(url).await.expect("Failed to connect");

        let _ = send_proof(&mut ws_stream, proof).await;

        while let Some(message) = ws_stream.next().await {
            match message {
                Ok(msg) => {
                    if msg.is_text() {
                        match msg.to_text() {
                            Ok(text) => {
                                if text.starts_with("best_hash:") {
                                    best_hash = text["best_hash:".len()..].to_string();
                                    break;
                                }
                            }
                            Err(e) => {
                                eprintln!("Failed to parse text message: {}", e);
                            }
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Error during the websocket communication: {}", e);
                    break;
                }
            }
        }

        let best_solution: ClientSolution =
            serde_json::from_str(&best_hash).expect("Failed to deserialize best hash");

        println!("Difficulty: {:?}", best_solution.difficulty);

        Solution::new(best_solution.hash.d, best_solution.nonce.to_le_bytes())
    }

    pub fn check_num_cores(&self, threads: u64) {
        // Check num threads
        let num_cores = num_cpus::get() as u64;
        if threads > num_cores {
            println!(
                "{} Number of threads ({}) exceeds available cores ({})",
                "WARNING".bold().yellow(),
                threads,
                num_cores
            );
        }
    }

    async fn should_reset(&self, config: Config) -> bool {
        let clock = get_clock(&self.rpc_client).await;
        config
            .last_reset_at
            .saturating_add(EPOCH_DURATION)
            .saturating_sub(5) // Buffer
            .le(&clock.unix_timestamp)
    }

    async fn get_cutoff(&self, proof: Proof, buffer_time: u64) -> u64 {
        let clock = get_clock(&self.rpc_client).await;
        proof
            .last_hash_at
            .saturating_add(60)
            .saturating_sub(buffer_time as i64)
            .saturating_sub(clock.unix_timestamp)
            .max(50) as u64
    }
}

fn find_bus() -> Pubkey {
    let i = rand::thread_rng().gen_range(0..BUS_COUNT);
    BUS_ADDRESSES[i]
}

async fn send_proof(
    ws_stream: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<TcpStream>,
    >,
    proof: Proof,
) -> Result<()> {
    let msg = Message::text(format!(
        "proof:{}",
        serde_json::to_string(&proof.to_bytes()).expect("Failed to serialize proof")
    ));
    ws_stream.send(msg).await?;
    Ok(())
}
