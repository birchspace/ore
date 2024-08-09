use std::{fmt::Formatter, sync::Arc};

use futures_util::stream::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};
use solana_client::client_error::Result;
use solana_sdk::{pubkey::Pubkey, signature::Signature, transaction::Transaction};
use solana_transaction_status::{Encodable, EncodedTransaction, UiTransactionEncoding};
use tokio::{sync::RwLock, task::JoinHandle};

use crate::constant;

#[derive(Debug, Deserialize)]
pub struct JitoResponse<T> {
    pub result: T,
}

async fn make_jito_request(method: &'static str, params: Value) -> eyre::Result<String> {
    let response = reqwest::Client::new()
        .post("https://ny.mainnet.block-engine.jito.wtf/api/v1/bundles")
        .header("Content-Type", "application/json")
        .json(&json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}))
        .send()
        .await;

    let response = match response {
        Ok(response) => response,
        Err(err) => eyre::bail!("fail to send request: {err}"),
    };

    let status = response.status();
    if !status.is_success() {
        let res_text: String = response.text().await?;

        eyre::bail!("status code: {status}, response: {res_text}");
    } else {
        let res: Value = response.json().await?;
        let res_text = res.get("result").and_then(Value::as_str).unwrap();

        Ok(res_text.to_string())
    }
}

pub async fn send_bundle(bundle: Vec<Transaction>) -> Result<Signature> {
    let signature = *bundle
        .first()
        .expect("empty bundle")
        .signatures
        .first()
        .expect("empty transaction");

    let bundle = bundle
        .into_iter()
        .map(|tx| match tx.encode(UiTransactionEncoding::Binary) {
            EncodedTransaction::LegacyBinary(b) => b,
            _ => panic!("impossible"),
        })
        .collect::<Vec<_>>();

    let _response = make_jito_request("sendBundle", json!([bundle])).await;

    Ok(signature)
}

pub fn build_bribe_ix(pubkey: &Pubkey, value: u64) -> solana_sdk::instruction::Instruction {
    solana_sdk::system_instruction::transfer(pubkey, constant::pick_jito_recipient(), value)
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct JitoTips {
    #[serde(rename = "landed_tips_25th_percentile")]
    pub p25_landed: f64,

    #[serde(rename = "landed_tips_50th_percentile")]
    pub p50_landed: f64,

    #[serde(rename = "landed_tips_75th_percentile")]
    pub p75_landed: f64,

    #[serde(rename = "landed_tips_95th_percentile")]
    pub p95_landed: f64,

    #[serde(rename = "landed_tips_99th_percentile")]
    pub p99_landed: f64,
}

impl std::fmt::Display for JitoTips {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "tips(p25={},p50={},p75={},p95={},p99={})",
            (self.p25_landed * 1e9f64) as u64,
            (self.p50_landed * 1e9f64) as u64,
            (self.p75_landed * 1e9f64) as u64,
            (self.p95_landed * 1e9f64) as u64,
            (self.p99_landed * 1e9f64) as u64
        )
    }
}

impl JitoTips {
    pub fn p75(&self) -> u64 {
        (self.p75_landed * 1e9f64) as u64
    }
}
