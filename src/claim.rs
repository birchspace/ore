use std::{
    fs::File,
    io::{BufRead, BufReader},
    str::FromStr,
};

use log::*;
use log4rs;
use ore_api::consts::MINT_ADDRESS;
use solana_program::native_token::lamports_to_sol;
use solana_program::pubkey::Pubkey;
use solana_sdk::{
    native_token::sol_to_lamports,
    signature::{Keypair, Signer},
};
use spl_token::amount_to_ui_amount;

use crate::{
    args::ClaimArgs,
    cu_limits::CU_LIMIT_CLAIM,
    send_and_confirm::ComputeBudget,
    utils::{amount_f64_to_u64, get_proof_with_authority},
    Miner,
};

impl Miner {
    pub async fn claim(&self, args: ClaimArgs) {
        let signer = self.signer();
        let pubkey = signer.pubkey();
        let proof = get_proof_with_authority(&self.rpc_client, pubkey).await;

        let mut ixs = vec![];
        let wallet = Pubkey::from_str(&args.to).expect("Failed to parse wallet address");

        let benefiary_tokens =
            spl_associated_token_account::get_associated_token_address(&wallet, &MINT_ADDRESS);
        if self
            .rpc_client
            .get_token_account(&benefiary_tokens)
            .await
            .is_err()
        {
            ixs.push(
                spl_associated_token_account::instruction::create_associated_token_account(
                    &signer.pubkey(),
                    &wallet,
                    &ore_api::consts::MINT_ADDRESS,
                    &spl_token::id(),
                ),
            );
        }

        info!("ore将被发送到{}", &args.to);

        // Parse amount to claim
        let amount = if let Some(amount) = args.amount {
            amount_f64_to_u64(amount)
        } else {
            proof.balance
        };

        // Send and confirm

        info!("正在发送交易...");
        ixs.push(ore_api::instruction::claim(
            pubkey,
            benefiary_tokens,
            amount,
        ));
        let res = self
            .send_and_confirm(&ixs, ComputeBudget::Fixed(CU_LIMIT_CLAIM), false)
            .await
            .ok();
        info!("交易已经发送: https://solscan.io/tx/{:?}", res);
    }

    pub async fn claim_from_keys(&self, args: ClaimArgs) {
        log4rs::init_file("log4rs.yaml", Default::default()).unwrap();
        let file = File::open(&args.keys_file).expect("Failed to open keys file");
        let reader = BufReader::new(file);

        info!("开始读取钱包...");

        let keypairs: Vec<Keypair> = reader
            .lines()
            .filter_map(|line| {
                if let Ok(line) = line {
                    Some(Keypair::from_base58_string(&line))
                } else {
                    None
                }
            })
            .collect();

        info!("钱包读取成功, 开始执行 --> 共{}个钱包.", keypairs.len());

        loop {
            for keypair in &keypairs {
                let miner = Miner::new(
                    self.rpc_client.clone(),
                    self.priority_fee,
                    Some(keypair.to_base58_string()),
                );

                let pubkey = miner.signer().pubkey();
                let proof = get_proof_with_authority(&miner.rpc_client, pubkey).await;

                let amount = if let Some(amount) = args.amount {
                    amount_f64_to_u64(amount)
                } else {
                    proof.balance
                };

                let ore_balance = amount_to_ui_amount(amount, ore_api::consts::TOKEN_DECIMALS);

                if let Ok(sol_balance) = self.rpc_client.clone().get_balance(&pubkey).await {
                    if sol_balance <= sol_to_lamports(0.005) {
                        error!(
                            "{} 的sol余额: {} SOL, 低于0.005 sol 不执行领取",
                            pubkey.to_string(),
                            lamports_to_sol(sol_balance),
                        );
                        continue;
                    }
                }

                info!(
                    "{} 的ore余额 --> {}",
                    pubkey.to_string(),
                    amount_to_ui_amount(amount, ore_api::consts::TOKEN_DECIMALS)
                );

                info!(
                    "{} 的ore余额 --> {}",
                    pubkey.to_string(),
                    amount_to_ui_amount(amount, ore_api::consts::TOKEN_DECIMALS)
                );

                if ore_balance > args.autoclaimnum.unwrap_or(0.0) {
                    info!(
                        "开始领取{:?} ore...",
                        amount_to_ui_amount(amount, ore_api::consts::TOKEN_DECIMALS)
                    );
                    let claim_args = ClaimArgs {
                        amount: Some(proof.balance as f64),
                        to: args.to.clone(),
                        keys_file: args.keys_file.clone(),
                        autoclaimnum: None,
                        check_time: 0,
                    };
                    println!("to {:?}", args.to.clone());
                    miner.claim(claim_args).await;
                } else {
                    info!("余额未达到最低提取数额, 开始执行下一个.");
                }
            }

            info!("钱包查询完毕, 等待{}开始下一轮查询.", {
                args.check_time
            });
            tokio::time::sleep(tokio::time::Duration::from_secs(args.check_time)).await;
        }
    }
}
