use bytemuck::from_bytes;
use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use log::{error, info, warn, LevelFilter};
use log4rs::append::console::ConsoleAppender;
use log4rs::append::file::FileAppender;
use log4rs::config::{Appender, Logger, Root};
use log4rs::encode::pattern::PatternEncoder;
use log4rs::{init_config, Config};
use ore_api::state::Proof;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::time;
use tokio_tungstenite::{accept_async, tungstenite::Message};

type Tx = futures_channel::mpsc::UnboundedSender<Message>;
type PeerMap = Arc<RwLock<HashMap<SocketAddr, (Tx, usize)>>>;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(long, value_name = "PORT", help = "Port to bind the server")]
    port: u64,
}

#[derive(Serialize, Deserialize, Debug)]
struct Solution {
    nonce: u64,
    difficulty: u32,
    hash: HashWrapper,
}

#[derive(Serialize, Deserialize, Debug)]
struct HashWrapper {
    h: Vec<u8>,
    d: [u8; 16],
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

struct ServerState {
    total_threads: AtomicUsize,
    peers: PeerMap,
    task_tx: mpsc::UnboundedSender<String>,
    proof: Mutex<Option<Proof>>,
    solutions: Mutex<Vec<Solution>>,
}

impl ServerState {
    fn new(task_tx: mpsc::UnboundedSender<String>) -> Self {
        ServerState {
            total_threads: AtomicUsize::new(0),
            peers: Arc::new(RwLock::new(HashMap::new())),
            task_tx,
            proof: Mutex::new(None),
            solutions: Mutex::new(Vec::new()),
        }
    }

    fn add_threads(&self, peer: SocketAddr, count: usize) {
        let mut peers = self.peers.write().unwrap();
        if let Some((_, existing_count)) = peers.get_mut(&peer) {
            self.total_threads
                .fetch_sub(*existing_count, Ordering::SeqCst);
            *existing_count = count;
        } else {
            peers.insert(peer, (futures_channel::mpsc::unbounded().0, count));
        }
        self.total_threads.fetch_add(count, Ordering::SeqCst);
    }

    fn add_solution(&self, solution: Solution) {
        let mut solutions = self.solutions.lock().unwrap();
        solutions.push(solution);
    }

    fn remove_peer(&self, peer: &SocketAddr) {
        let mut peers = self.peers.write().unwrap();
        if let Some((_, count)) = peers.remove(peer) {
            self.total_threads.fetch_sub(count, Ordering::SeqCst);
        }
    }

    fn get_peers(&self) -> Vec<(Tx, usize)> {
        self.peers.read().unwrap().values().cloned().collect()
    }

    fn process_solutions(&self) {
        let mut solutions = self.solutions.lock().unwrap();
        if let Some(best_solution) = solutions.iter().max_by_key(|s| s.difficulty) {
            let peers = self.peers.read().unwrap();
            let current_time = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();

            if let Some((peer, (_, threads))) = peers.iter().find(|(_, (tx, _))| {
                let serialized_solution = serde_json::to_string(&best_solution).unwrap();
                tx.unbounded_send(Message::Text(format!("best_hash:{}", serialized_solution)))
                    .is_ok()
            }) {
                let difficulty = best_solution.difficulty;
                let log_message = format!(
                    "Time: {}, Node: {}, Difficulty: {}, Threads: {}",
                    current_time, peer, difficulty, threads
                );

                // 记录到 difficulty.log
                log::info!(target: "difficulty_logger", "{}", log_message);

                // 如果难度超过 23，则记录到 difficulty_high.log
                if difficulty > 23 {
                    log::info!(target: "difficulty_high_logger", "{}", log_message);
                }
            }

            info!("最佳 solution 选定: {:?}", best_solution);
        }
        info!("清空所有 solution");
        solutions.clear();
    }

    fn log_server_status(&self) {
        let peers = self.peers.read().unwrap();
        let total_threads = self.total_threads.load(Ordering::SeqCst);
        let num_peers = peers.len();

        info!(
            "服务器状态: 总线程数: {}, 连接的节点数: {}",
            total_threads, num_peers
        );

        for (peer, (_, threads)) in peers.iter() {
            info!("节点 {}: 分配的线程数: {}", peer, threads);
        }
    }
}

const TIMER: u64 = 45;

async fn accept_connection(state: Arc<ServerState>, peer: SocketAddr, stream: TcpStream) {
    info!("节点 {} 连接成功", peer);
    if let Err(e) = handle_connection(&state, peer, stream).await {
        error!("处理节点 {} 连接时出错: {}", peer, e);
    }
    info!("节点 {} 断开连接", peer);
    state.remove_peer(&peer);
}

async fn handle_connection(
    state: &Arc<ServerState>,
    peer: SocketAddr,
    stream: TcpStream,
) -> Result<(), tokio_tungstenite::tungstenite::Error> {
    let ws_stream = accept_async(stream).await.expect("接受连接失败");

    let (tx, mut rx) = futures_channel::mpsc::unbounded();
    {
        let mut peers = state.peers.write().unwrap();
        peers.insert(peer, (tx.clone(), 0));
    }

    let (mut outgoing, incoming) = ws_stream.split();
    let mut incoming = incoming.fuse();

    tokio::spawn(async move {
        while let Some(msg) = rx.next().await {
            outgoing.send(msg).await.unwrap();
        }
    });

    while let Some(msg) = incoming.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                handle_text_message(&state, &peer, text).await;
            }
            Ok(Message::Binary(_)) => {}
            Ok(Message::Close(_)) => {
                state.remove_peer(&peer);
                info!("节点 {} 断开连接", peer);
                break;
            }
            Err(e) => {
                error!("处理节点 {} 消息时出错: {}", peer, e);
                state.remove_peer(&peer);
                break;
            }
            _ => {}
        }
    }

    Ok(())
}

async fn handle_text_message(state: &Arc<ServerState>, peer: &SocketAddr, text: String) {
    if text.starts_with("threads:") {
        if let Some(threads) = text[8..].parse::<usize>().ok() {
            state.add_threads(*peer, threads);
            let total_threads = state.total_threads.load(Ordering::SeqCst);
            info!(
                "节点 {} 增加线程数: {}, 当前总线程数: {}",
                peer, threads, total_threads
            );
        }
    } else if text.starts_with("hash:") {
        let json_str = &text[5..];
        match serde_json::from_str::<serde_json::Value>(json_str) {
            Ok(value) => match serde_json::from_value::<Solution>(value) {
                Ok(solution) => {
                    let peers = state.peers.read().unwrap();
                    let node_threads = peers.get(peer).map(|(_, threads)| *threads).unwrap_or(0);

                    info!(
                        "接收到来自节点 {} 的 solution, 该节点的线程数: {}",
                        peer, node_threads
                    );

                    state.add_solution(solution);
                }
                Err(err) => {
                    error!("解析 solution 失败: {}", err);
                }
            },
            Err(err) => {
                error!("解析 JSON 失败: {}", err);
            }
        }
    } else if text.starts_with("proof:") {
        let proof_vec = serde_json::from_str::<Vec<u8>>(&text["proof:".len()..]).unwrap();
        let proof: &Proof = from_bytes(&proof_vec);
        {
            let mut stored_proof = state.proof.lock().unwrap();
            *stored_proof = Some(proof.clone());
        }
        info!("接收到来自节点 {} 的 proof, 准备开始分发任务", peer);
        state.task_tx.send("start".to_string()).unwrap();
    } else {
        warn!("未知消息来自节点 {}: {:?}", peer, text);
    }
    state.log_server_status();
}

async fn distribute_tasks(state: Arc<ServerState>, proof: &Proof) {
    info!("开始分发任务");
    let peers = state.get_peers();
    let total_threads: usize = peers.iter().map(|(_, threads)| *threads).sum();
    let mut start_nonce: u64 = 0;

    for (tx, threads) in peers {
        let nonce_range = (u64::MAX / total_threads as u64) * threads as u64;
        let task_message = TaskMessage {
            task: Task {
                authority: proof.authority.to_string(),
                balance: proof.balance as i64,
                challenge: proof.challenge.to_vec(),
                last_hash: proof.last_hash.to_vec(),
                last_hash_at: proof.last_hash_at,
                last_stake_at: proof.last_stake_at,
                miner: proof.miner.to_string(),
                total_hashes: proof.total_hashes as i64,
                total_rewards: proof.total_rewards as i64,
            },
            start_nonce,
            nonce_range,
        };

        let task_with_nonce = serde_json::to_string(&task_message).expect("序列化任务消息失败");
        tx.unbounded_send(Message::Text(task_with_nonce)).unwrap();
        start_nonce += nonce_range;
    }

    info!("任务分发完毕, 启动定时器");
    start_timer(state.clone()).await;
}

async fn start_timer(state: Arc<ServerState>) {
    let duration = Duration::from_secs(TIMER);
    info!("启动 {} 秒定时器", duration.as_secs());

    time::sleep(duration).await;

    info!("定时器结束, 开始处理 solutions");
    state.process_solutions();
}

async fn generate_tasks(state: Arc<ServerState>, mut task_rx: mpsc::UnboundedReceiver<String>) {
    while let Some(message) = task_rx.recv().await {
        if message == "start" {
            let proof = {
                let stored_proof = state.proof.lock().unwrap();
                stored_proof.clone()
            };

            if let Some(proof) = proof {
                distribute_tasks(state.clone(), &proof).await;
            } else {
                error!("没有可用的 proof 来分发任务");
            }
        }
    }
}

fn setup_logging() {
    let stdout = ConsoleAppender::builder().build();

    // 普通请求日志
    let requests = FileAppender::builder()
        .encoder(Box::new(PatternEncoder::new("{d} - {m}{n}")))
        .build("log/requests.log")
        .unwrap();

    // difficulty记录日志
    let difficulty_file = FileAppender::builder()
        .encoder(Box::new(PatternEncoder::new("{d} - Node: {m}{n}")))
        .build("log/difficulty.log")
        .unwrap();

    // difficulty_high记录日志
    let difficulty_high_file = FileAppender::builder()
        .encoder(Box::new(PatternEncoder::new("{d} - Node: {m}{n}")))
        .build("log/difficulty_high.log")
        .unwrap();

    // 配置日志
    let config = Config::builder()
        .appender(Appender::builder().build("stdout", Box::new(stdout)))
        .appender(Appender::builder().build("requests", Box::new(requests)))
        .appender(Appender::builder().build("difficulty_file", Box::new(difficulty_file)))
        .appender(Appender::builder().build("difficulty_high_file", Box::new(difficulty_high_file)))
        .logger(
            Logger::builder()
                .appender("requests")
                .additive(false)
                .build("app::requests", LevelFilter::Info),
        )
        .logger(
            Logger::builder()
                .appender("difficulty_file")
                .additive(false)
                .build("difficulty_logger", LevelFilter::Info),
        )
        .logger(
            Logger::builder()
                .appender("difficulty_high_file")
                .additive(false)
                .build("difficulty_high_logger", LevelFilter::Info),
        )
        .build(Root::builder().appender("stdout").build(LevelFilter::Info))
        .unwrap();

    init_config(config).unwrap();
}


#[tokio::main]
async fn main() {
    setup_logging();

    // 解析命令行参数
    let args = Args::parse();
    let addr = format!("0.0.0.0:{}", args.port);

    // 绑定地址和端口
    let listener = TcpListener::bind(&addr).await.expect("无法监听");

    info!("服务器启动完成, 监听端口: {}", args.port);

    let (task_tx, task_rx) = mpsc::unbounded_channel();
    let state = Arc::new(ServerState::new(task_tx));

    // 输出当前的服务状态信息
    state.log_server_status();

    tokio::spawn(generate_tasks(state.clone(), task_rx));

    // 监听并接受新的连接
    while let Ok((stream, _)) = listener.accept().await {
        let peer = stream.peer_addr().expect("连接的流应具有对等地址");
        info!("接受到新连接: {}", peer);
        tokio::spawn(accept_connection(state.clone(), peer, stream));
    }
}
