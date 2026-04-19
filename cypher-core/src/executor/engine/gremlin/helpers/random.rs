//! Java-compatible LCG used by Gremlin's `sample` / `coin` steps so that
//! seeded results match the JVM Tinkerpop reference implementation.

pub(in crate::executor::engine::gremlin) struct JavaCompatibleRandom {
    seed: u64,
}

impl JavaCompatibleRandom {
    const MULTIPLIER: u64 = 0x5DEECE66D;
    const ADDEND: u64 = 0xB;
    const MASK: u64 = (1u64 << 48) - 1;

    pub(in crate::executor::engine::gremlin) fn new(seed: i64) -> Self {
        Self {
            seed: ((seed as u64) ^ Self::MULTIPLIER) & Self::MASK,
        }
    }

    pub(in crate::executor::engine::gremlin) fn next(&mut self, bits: u32) -> u32 {
        self.seed = (self
            .seed
            .wrapping_mul(Self::MULTIPLIER)
            .wrapping_add(Self::ADDEND))
            & Self::MASK;
        (self.seed >> (48 - bits)) as u32
    }

    pub(in crate::executor::engine::gremlin) fn next_double(&mut self) -> f64 {
        let high = self.next(26) as u64;
        let low = self.next(27) as u64;
        ((high << 27) | low) as f64 / ((1u64 << 53) as f64)
    }
}
