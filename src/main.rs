use drillx::Hash;
use find::find_hash_par;
use futures_util::{SinkExt, StreamExt};
use log::*;
use log4rs;
use ore_api::state::Proof;
use serde::{Deserialize, Serialize};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Result;
use tokio_tungstenite::{connect_async, tungstenite::Message};
mod find;
use clap::{arg, command, Parser};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(long, value_name = "IP", help = "ip", global = true)]
    ip: String,

    #[arg(long, value_name = "PORT", help = "port", global = true)]
    port: u64,

    #[arg(long, value_name = "THREADS", help = "threads", global = true)]
    threads: u64,

    #[arg(long, value_name = "CUTOFF_TIME", help = "cutoff_time", global = true)]
    cutoff_time: u64,
}

#[derive(Serialize, Deserialize, Debug)]
struct Task {
    authority: String,
    balance: i64,
    challenge: Vec<u8>,
    last_hash: Vec<u8>,
    last_hash_at: i64,
    last_stake_at: i64,
    miner: String,
    total_hashes: i64,
    total_rewards: i64,
}

#[derive(Serialize, Deserialize, Debug)]
struct TaskMessage {
    task: Task,
    start_nonce: u64,
    nonce_range: u64,
}

#[derive(Serialize, Deserialize, Debug)]
struct HashWrapper {
    h: Vec<u8>,
    d: [u8; 16],
}

impl From<Hash> for HashWrapper {
    fn from(hash: Hash) -> Self {
        HashWrapper {
            h: hash.h.to_vec(),
            d: hash.d,
        }
    }
}

impl From<HashWrapper> for Hash {
    fn from(wrapper: HashWrapper) -> Self {
        Hash {
            h: wrapper.h.try_into().expect("Invalid length"),
            d: wrapper.d,
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
struct Solution {
    nonce: u64,
    difficulty: u32,
    hash: HashWrapper,
}

impl From<Task> for Proof {
    fn from(task: Task) -> Self {
        Proof {
            authority: task.authority.parse().expect("Invalid authority pubkey"),
            balance: task.balance as u64,
            challenge: task.challenge.try_into().expect("Invalid challenge length"),
            last_hash: task.last_hash.try_into().expect("Invalid last_hash length"),
            last_hash_at: task.last_hash_at,
            last_stake_at: task.last_stake_at,
            miner: task.miner.parse().expect("Invalid miner pubkey"),
            total_hashes: task.total_hashes as u64,
            total_rewards: task.total_rewards as u64,
        }
    }
}

async fn get_ash(
    ws_stream: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<TcpStream>,
    >,
    msg: Message,
    threads: u64,
    cutoff_time: u64,
) {
    match msg {
        Message::Text(text) => match serde_json::from_str::<TaskMessage>(&text) {
            Ok(task_message) => {
                let proof: Proof = task_message.task.into();
                info!("收到任务, 正在计算...");
                if let Some((nonce, difficulty, hash)) =
                    find_hash_par(proof, cutoff_time, threads, 1).await
                {
                    info!("计算难度为 {} --> 计算完成, 发送hash...", difficulty);
                    let solution = Solution {
                        nonce,
                        difficulty,
                        hash: hash.into(),
                    };
                    send_solution(ws_stream, Some(solution))
                        .await
                        .expect("发送hash失败");
                } else {
                    warn!("未找到合适的hash, 发送空hash...");
                    send_solution(ws_stream, None).await.expect("发送hash失败");
                }
            }
            Err(err) => {
                error!("未知错误: {}", err);
            }
        },
        Message::Binary(_bin) => {
            println!("\n");
        }
        _ => {
            println!("\n");
        }
    }
}

async fn send_thread_info(
    ws_stream: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<TcpStream>,
    >,
    threads: u64,
) -> Result<()> {
    let thread_info = format!("threads:{}", threads);
    let msg = Message::text(thread_info.to_string());
    ws_stream.send(msg).await?;

    info!("线程信息已发送");
    Ok(())
}

async fn send_solution(
    ws_stream: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<TcpStream>,
    >,
    solution: Option<Solution>,
) -> Result<()> {
    let solution_info = serde_json::to_string(&solution).expect("hash序列化失败");
    let msg = Message::text(format!("hash:{}", solution_info));
    ws_stream.send(msg).await?;
    info!("Hash 发送完毕 {}", solution_info);
    Ok(())
}

#[tokio::main]
async fn main() {
    env_logger::init();

    let args = Args::parse();

    let url = format!("ws://{}:{}", args.ip, args.port);

    log4rs::init_file("log4rs.yaml", Default::default()).unwrap();
    info!("已连接到 {}", url);

    let (mut ws_stream, _) = connect_async(url).await.expect("连接失败");

    // 发送线程数
    send_thread_info(&mut ws_stream, args.threads)
        .await
        .expect("发送线程信息失败");

    // 长连接, 获取proof
    while let Some(message) = ws_stream.next().await {
        match message {
            Ok(msg) => get_ash(&mut ws_stream, msg, args.threads, args.cutoff_time).await,
            Err(e) => {
                error!("WebSocket 通信时出错: {}", e);
                break;
            }
        }
    }
    ws_stream.close(None).await.expect("关闭连接失败");
    warn!("连接已关闭.");
}
