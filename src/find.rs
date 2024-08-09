use drillx::{equix, Hash};
use ore_api::state::Proof;
use std::time::Instant;

pub async fn find_hash_par(
    proof: Proof,
    cutoff_time: u64,
    threads: u64,
) -> Option<(u64, u32, Hash)> {
    let rt = tokio::runtime::Handle::current();
    let handles: Vec<_> = (0..threads)
        .map(|i| {
            rt.spawn_blocking({
                let proof = proof.clone();
                let mut memory = equix::SolverMemory::new();
                move || {
                    let timer = Instant::now();
                    let mut nonce = u64::MAX.saturating_div(threads).saturating_mul(i);
                    let mut best_nonce = nonce;
                    let mut best_difficulty = 0;
                    let mut best_hash = Hash::default();

                    loop {
                        // 检查是否超过了cutoff_time
                        if timer.elapsed().as_secs() >= cutoff_time {
                            break;
                        }

                        // 计算哈希值并更新最佳难度
                        if let Ok(hx) = drillx::hash_with_memory(
                            &mut memory,
                            &proof.challenge,
                            &nonce.to_le_bytes(),
                        ) {
                            let difficulty = hx.difficulty();
                            if difficulty > best_difficulty {
                                best_nonce = nonce;
                                best_difficulty = difficulty;
                                best_hash = hx;
                            }
                        }

                        nonce += 1;
                    }

                    // 返回线程找到的最佳结果
                    (best_nonce, best_difficulty, best_hash)
                }
            })
        })
        .collect();

    let joined = futures::future::join_all(handles).await;

    // 合并所有线程的结果，找到全局最佳结果
    let (best_nonce, best_difficulty, best_hash) = joined.into_iter().fold(
        (0, 0, Hash::default()),
        |(best_nonce, best_difficulty, best_hash), h| {
            if let Ok((nonce, difficulty, hash)) = h {
                if difficulty > best_difficulty {
                    (nonce, difficulty, hash)
                } else {
                    (best_nonce, best_difficulty, best_hash)
                }
            } else {
                (best_nonce, best_difficulty, best_hash)
            }
        },
    );

    Some((best_nonce, best_difficulty, best_hash))
}
