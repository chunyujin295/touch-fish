//! 噪声引擎：所有"假的"数字、广告、评论、推荐位都由这里生成。
//! 关键约束是**可复现**——同一本书 + 同一章 + 同一皮肤 + 同一主题包，
//! 生成的噪声必须完全一致，否则每次刷新广告评论全变，反而露馅。

pub fn seed_of(parts: &[&str]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for p in parts {
        for b in p.as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100_0000_01b3);
        }
        h ^= 0x9e37_79b9_7f4a_7c15;
    }
    h
}

pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(if seed == 0 { 0x9e37_79b9_7f4a_7c15 } else { seed })
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    #[allow(dead_code)]
    pub fn pick<'a, T>(&mut self, v: &'a [T]) -> &'a T {
        let i = (self.next_u64() % v.len() as u64) as usize;
        &v[i]
    }

    pub fn range(&mut self, lo: usize, hi: usize) -> usize {
        if hi <= lo {
            return lo;
        }
        lo + (self.next_u64() % (hi - lo + 1) as u64) as usize
    }

    /// 不放回抽样
    #[allow(dead_code)]
    pub fn take<T: Clone>(&mut self, v: &[T], n: usize) -> Vec<T> {
        let mut pool: Vec<T> = v.to_vec();
        let mut out = Vec::new();
        while out.len() < n && !pool.is_empty() {
            let i = (self.next_u64() % pool.len() as u64) as usize;
            out.push(pool.remove(i));
        }
        out
    }
}
