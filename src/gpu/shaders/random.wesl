// Random numbers.

// PCG hash (Jarzynski & Olano, "Hash Functions for GPU Rendering", 2020).
fn pcg_hash(v: u32) -> u32 {
    let state = v * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

// Uniform in [0, 1).
fn rand(state: ptr<function, u32>) -> f32 {
    *state = pcg_hash(*state);
    return f32(*state >> 8u) / 16777216.0;
}
