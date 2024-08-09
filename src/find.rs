use ore_api::state::Proof;

use drillx::{equix, Hash};

use std::time::Instant;

pub async fn find_hash_par(
    proof: Proof,
    cutoff_time: u64,
    threads: u64,
    _min_difficulty: u32,
) -> Option<(u64, u32, Hash)> {
    let handles: Vec<_> = (0..threads)
        .map(|i| {
            std::thread::spawn({
                let proof = proof.clone();

                let mut memory = equix::SolverMemory::new();
                move || {
                    let timer = Instant::now();
                    let mut nonce = u64::MAX.saturating_div(threads).saturating_mul(i);
                    let mut best_nonce = nonce;
                    let mut best_difficulty = 0;
                    let mut best_hash = Hash::default();
                    loop {
                        if let Ok(hx) = drillx::hash_with_memory(
                            &mut memory,
                            &proof.challenge,
                            &nonce.to_le_bytes(),
                        ) {
                            let difficulty = hx.difficulty();
                            if difficulty.gt(&best_difficulty) {
                                best_nonce = nonce;
                                best_difficulty = difficulty;
                                best_hash = hx;
                            }
                        }

                        if timer.elapsed().as_secs().ge(&cutoff_time) {
                            break;
                        }

                        nonce += 1;
                    }

                    (best_nonce, best_difficulty, best_hash)
                }
            })
        })
        .collect();

    let mut best_nonce = 0;
    let mut best_difficulty = 0;
    let mut best_hash = Hash::default();
    for h in handles {
        if let Ok((nonce, difficulty, hash)) = h.join() {
            if difficulty > best_difficulty {
                best_difficulty = difficulty;
                best_nonce = nonce;
                best_hash = hash;
            }
        }
    }

    Some((best_nonce, best_difficulty, best_hash))
}
