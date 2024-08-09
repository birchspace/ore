use std::time::Duration;

use colored::*;
use solana_client::client_error::{ClientError, ClientErrorKind, Result as ClientResult};
use solana_program::{
    instruction::Instruction,
    native_token::{lamports_to_sol, sol_to_lamports},
};

use solana_rpc_client::spinner;
use solana_sdk::{
    signature::{Signature, Signer},
    transaction::Transaction,
};
use solana_transaction_status::TransactionConfirmationStatus;

use crate::{
    jito::{self, send_bundle},
    Miner,
};

const GATEWAY_RETRIES: usize = 20;
const CONFIRM_RETRIES: usize = 1;

const CONFIRM_DELAY: u64 = 200;
const GATEWAY_DELAY: u64 = 1000;

const MIN_SOL_BALANCE: f64 = 0.005;

const _SIMULATION_RETRIES: usize = 4;

pub enum ComputeBudget {
    Dynamic,
    Fixed(u32),
}

impl Miner {
    pub async fn send_and_confirm(
        &self,
        ixs: &[Instruction],
        _compute_budget: ComputeBudget,
        _skip_confirm: bool,
    ) -> ClientResult<Signature> {
        let signer = self.signer();
        let client = self.rpc_client.clone();

        let tip = self.priority_fee;

        let progress_bar = spinner::new_progress_bar();

        // Return error, if balance is zero
        if let Ok(balance) = client.get_balance(&signer.pubkey()).await {
            if balance <= sol_to_lamports(MIN_SOL_BALANCE) {
                panic!(
                    "{} Insufficient balance: {} SOL\nPlease top up with at least {} SOL",
                    "ERROR".bold().red(),
                    lamports_to_sol(balance),
                    MIN_SOL_BALANCE
                );
            }
        }

        // Set compute units
        let mut final_ixs = vec![];

        final_ixs.extend_from_slice(ixs);

        // add jito
        final_ixs.push(jito::build_bribe_ix(&self.signer().pubkey(), tip));

        let mut tx = Transaction::new_with_payer(&final_ixs, Some(&signer.pubkey()));

        // Sign tx
        let (hash, _slot) = client
            .get_latest_blockhash_with_commitment(self.rpc_client.commitment())
            .await
            .unwrap();
        tx.sign(&[&signer], hash);

        // send tx
        let mut bundle = Vec::new();
        bundle.push(tx);

        // Submit tx
        let mut attempts = 0;
        loop {
            progress_bar.set_message(format!("Submitting transaction... (attempt {})", attempts));

            match send_bundle(bundle.clone()).await {
                Ok(sig) => {
                    // 等待十秒
                    tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
                    // Confirm the tx landed\
                    for _ in 0..CONFIRM_RETRIES {
                        std::thread::sleep(Duration::from_millis(CONFIRM_DELAY));
                        match client.get_signature_statuses(&[sig]).await {
                            Ok(signature_statuses) => {
                                for status in signature_statuses.value {
                                    if let Some(status) = status {
                                        if let Some(err) = status.err {
                                            progress_bar.finish_with_message(format!(
                                                "{}: {}",
                                                "ERROR".bold().red(),
                                                err
                                            ));
                                            return Err(ClientError {
                                                request: None,
                                                kind: ClientErrorKind::Custom(err.to_string()),
                                            });
                                        }
                                        if let Some(confirmation) = status.confirmation_status {
                                            match confirmation {
                                                TransactionConfirmationStatus::Processed
                                                | TransactionConfirmationStatus::Confirmed
                                                | TransactionConfirmationStatus::Finalized => {
                                                    progress_bar.finish_with_message(format!(
                                                        "{} {}",
                                                        "OK".bold().green(),
                                                        sig
                                                    ));
                                                    return Ok(sig);
                                                }
                                            }
                                        }
                                    }
                                }
                            }

                            // Handle confirmation errors
                            Err(err) => {
                                progress_bar.set_message(format!(
                                    "{}: {}",
                                    "ERROR".bold().red(),
                                    err.kind().to_string()
                                ));
                            }
                        }
                    }
                }

                // Handle submit errors
                Err(err) => {
                    progress_bar.set_message(format!(
                        "{}: {}",
                        "ERROR".bold().red(),
                        err.kind().to_string()
                    ));
                }
            }

            // Retry
            std::thread::sleep(Duration::from_millis(GATEWAY_DELAY));
            attempts += 1;
            if attempts > GATEWAY_RETRIES {
                progress_bar.finish_with_message(format!("{}: Max retries", "ERROR".bold().red()));
                return Err(ClientError {
                    request: None,
                    kind: ClientErrorKind::Custom("Max retries".into()),
                });
            }
        }
    }
}
