use num_bigint::{BigInt, BigUint};
use num_integer::Integer;
use num_traits::{One, Zero};
use rand::Rng;

use crate::math::{modpow_uint_int, root_factor, shamir_trick, ModInverse, extended_gcd};
use crate::primes::generate_primes;
use crate::proofs;
use crate::traits::*;

#[derive(Debug, Clone)]
pub struct RsaAccumulator {
    lambda: usize,
    /// Generator
    g: BigUint,
    /// n = pq
    n: BigUint,

    // current accumulator state
    a_t: BigUint,

    // prod of the current set
    s: BigUint,
}

impl RsaAccumulator {
    /// Returns the current public state.
    pub fn state(&self) -> &BigUint {
        &self.a_t
    }
}

impl StaticAccumulator for RsaAccumulator {
    fn setup(rng: &mut impl Rng, lambda: usize) -> Self {
        // Generate n = p q, |n| = lambda
        // This is a trusted setup, as we do know `p` and `q`, even though
        // we choose not to store them.

        let (n, _, _, g) = generate_primes(rng, lambda).unwrap();

        RsaAccumulator {
            lambda,
            a_t: g.clone(),
            g,
            n,
            s: BigUint::one(),
        }
    }

    #[inline]
    fn add(&mut self, x: &BigUint) {
        debug_assert!(
            self.g.clone().modpow(&self.s, &self.n) == self.a_t,
            "invalid state - pre add"
        );

        // assumes x is already a prime
        self.s *= x;
        self.a_t = self.a_t.modpow(x, &self.n);
    }

    #[inline]
    fn mem_wit_create(&self, x: &BigUint) -> BigUint {
        debug_assert!(
            self.g.clone().modpow(&self.s, &self.n) == self.a_t,
            "invalid state"
        );

        let (s, r) = self.s.clone().div_rem(x);
        debug_assert!(r.is_zero(), "x was not a valid member of s");

        self.g.clone().modpow(&s, &self.n)
    }

    #[inline]
    fn ver_mem(&self, w: &BigUint, x: &BigUint) -> bool {
        w.modpow(x, &self.n) == self.a_t
    }
}

impl DynamicAccumulator for RsaAccumulator {
    fn del(&mut self, x: &BigUint) -> Option<()> {
        let old_s = self.s.clone();
        self.s /= x;

        if self.s == old_s {
            return None;
        }

        self.a_t = self.g.clone().modpow(&self.s, &self.n);
        Some(())
    }
}

impl UniversalAccumulator for RsaAccumulator {
    fn non_mem_wit_create(&self, x: &BigUint) -> (BigUint, BigInt) {
        // s* <- \prod_{s\in S} s
        let s_star = &self.s;

        // a, b <- Bezout(x, s*)
        let (_, a, b) = extended_gcd(x, s_star);
        let d = modpow_uint_int(&self.g, &a, &self.n).expect("prime");

        (d, b)
    }

    fn ver_non_mem(&self, w: &(BigUint, BigInt), x: &BigUint) -> bool {
        let (d, b) = w;

        // A^b
        let a_b = modpow_uint_int(&self.a_t, b, &self.n).expect("prime");
        // d^x
        let d_x = d.modpow(x, &self.n);

        // d^x A^b == g
        (d_x * &a_b) % &self.n == self.g
    }
}

impl BatchedAccumulator for RsaAccumulator {
    fn batch_add(&mut self, xs: &[BigUint]) -> BigUint {
        let mut x_star = BigUint::one();
        for x in xs {
            x_star *= x;
            self.s *= x;
        }

        let a_t = self.a_t.clone();
        self.a_t = self.a_t.modpow(&x_star, &self.n);

        proofs::ni_poe_prove(&x_star, &a_t, &self.a_t, &self.n)
    }

    fn ver_batch_add(&self, w: &BigUint, a_t: &BigUint, xs: &[BigUint]) -> bool {
        let mut x_star = BigUint::one();
        for x in xs {
            x_star *= x
        }

        proofs::ni_poe_verify(&x_star, a_t, &self.a_t, &w, &self.n)
    }

    fn batch_del(&mut self, pairs: &[(BigUint, BigUint)]) -> Option<BigUint> {
        if pairs.is_empty() {
            return None;
        }
        let mut pairs = pairs.iter();
        let a_t = self.a_t.clone();

        let (x0, w0) = pairs.next().unwrap();
        let mut x_star = x0.clone();
        let mut new_a_t = w0.clone();

        for (xi, wi) in pairs {
            new_a_t = shamir_trick(&new_a_t, wi, &x_star, xi, &self.n).unwrap();
            x_star *= xi;
            // for now this is not great, depends on this impl, not on the general design
            self.s /= xi;
        }

        self.a_t = new_a_t;

        Some(proofs::ni_poe_prove(&x_star, &self.a_t, &a_t, &self.n))
    }

    fn ver_batch_del(&self, w: &BigUint, a_t: &BigUint, xs: &[BigUint]) -> bool {
        let mut x_star = BigUint::one();
        for x in xs {
            x_star *= x
        }

        proofs::ni_poe_verify(&x_star, &self.a_t, a_t, &w, &self.n)
    }

    fn del_w_mem(&mut self, w: &BigUint, x: &BigUint) -> Option<()> {
        if !self.ver_mem(w, x) {
            return None;
        }

        self.s /= x;
        // w is a_t without x, so need to recompute
        self.a_t = w.clone();

        Some(())
    }

    #[inline]
    fn create_all_mem_wit(&self, s: &[BigUint]) -> Vec<BigUint> {
        root_factor(&self.g, &s, &self.n)
    }

    fn agg_mem_wit(
        &self,
        w_x: &BigUint,
        w_y: &BigUint,
        x: &BigUint,
        y: &BigUint,
    ) -> (BigUint, BigUint) {
        // TODO: check this matches, sth is not quite right in the paper here
        let w_xy = shamir_trick(w_x, w_y, x, y, &self.n).unwrap();
        let xy = x.clone() * y;

        debug_assert!(
            w_xy.modpow(&xy, &self.n) == self.a_t,
            "invalid shamir trick"
        );

        let pi = proofs::ni_poe_prove(&xy, &w_xy, &self.a_t, &self.n);

        (w_xy, pi)
    }

    fn ver_agg_mem_wit(&self, w_xy: &BigUint, pi: &BigUint, x: &BigUint, y: &BigUint) -> bool {
        let xy = x.clone() * y;
        proofs::ni_poe_verify(&xy, w_xy, &self.a_t, pi, &self.n)
    }

    fn mem_wit_create_star(&self, x: &BigUint) -> (BigUint, BigUint) {
        let w_x = self.mem_wit_create(x);
        debug_assert!(self.a_t != w_x, "{} was not a member", x);
        let p = proofs::ni_poe_prove(x, &w_x, &self.a_t, &self.n);

        (w_x, p)
    }

    fn ver_mem_star(&self, x: &BigUint, pi: &(BigUint, BigUint)) -> bool {
        proofs::ni_poe_verify(x, &pi.0, &self.a_t, &pi.1, &self.n)
    }

    fn mem_wit_x(
        &self,
        _other: &BigUint,
        w_x: &BigUint,
        w_y: &BigUint,
        _x: &BigUint,
        _y: &BigUint,
    ) -> BigUint {
        (w_x * w_y) % &self.n
    }

    fn ver_mem_x(&self, other: &BigUint, pi: &BigUint, x: &BigUint, y: &BigUint) -> bool {
        // assert x and y are coprime
        let (q, _, _) = extended_gcd(x, y);
        if !q.is_one() {
            return false;
        }

        // A_1^y
        let rhs_a = self.a_t.modpow(y, &self.n);
        // A_2^x
        let rhs_b = other.modpow(x, &self.n);

        // A_1^y * A_2^x
        let rhs = (rhs_a * rhs_b) % &self.n;
        // pi^{x * y}
        let lhs = pi.modpow(&(x.clone() * y), &self.n);

        lhs == rhs
    }

    fn non_mem_wit_create_star(
        &self,
        x: &BigUint,
    ) -> (BigUint, BigUint, (BigUint, BigUint, BigInt), BigUint) {
        let g = &self.g;
        let n = &self.n;

        // a, b <- Bezout(x, s_star)
        let (_, a, b) = extended_gcd(x, &self.s);

        // d <- g^a
        let d = modpow_uint_int(g, &a, n).expect("invalid state");
        // v <- A^b
        let v = modpow_uint_int(&self.a_t, &b, n).expect("invalid state");

        // pi_d <- NI-PoKE2(b, A, v)
        let pi_d = proofs::ni_poke2_prove(b, &self.a_t, &v, n);

        // k <- g * v^-1
        let k = (g * v.clone().mod_inverse(n).expect("invalid state")) % n;

        // pi_g <- NI-PoE(x, d, g * v^-1)
        let pi_g = proofs::ni_poe_prove(x, &d, &k, n);

        // return {d, v, pi_d, pi_g}
        (d, v, pi_d, pi_g)
    }

    fn ver_non_mem_star(
        &self,
        x: &BigUint,
        pi: &(BigUint, BigUint, (BigUint, BigUint, BigInt), BigUint),
    ) -> bool {
        let g = &self.g;
        let n = &self.n;

        let (d, v, pi_d, pi_g) = pi;

        // verify NI-PoKE2
        if !proofs::ni_poke2_verify(&self.a_t, &v, pi_d, n) {
            return false;
        }

        // verify NI-PoE
        let k = (g * v.clone().mod_inverse(n).expect("invalid state")) % n;

        if !proofs::ni_poe_verify(x, d, &k, pi_g, n) {
            return false;
        }

        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use num_bigint::Sign;
    use num_traits::FromPrimitive;
    use rand::{SeedableRng, XorShiftRng};
     use crate::math::prime_rand::RandPrime;

    #[test]
    fn test_static() {
        let rng = &mut XorShiftRng::from_seed([0u8; 16]);

        for _ in 0..100 {
            let lambda = 256; // insecure, but faster tests
            let mut acc = RsaAccumulator::setup(rng, lambda);

            let xs = (0..5).map(|_| rng.gen_prime(lambda)).collect::<Vec<_>>();

            for x in &xs {
                acc.add(x);
            }

            for x in &xs {
                let w = acc.mem_wit_create(x);
                assert!(acc.ver_mem(&w, x));
            }
        }
    }

    #[test]
    fn test_dynamic() {
        let rng = &mut XorShiftRng::from_seed([0u8; 16]);

        for _ in 0..20 {
            let lambda = 256; // insecure, but faster tests
            let mut acc = RsaAccumulator::setup(rng, lambda);

            let xs = (0..5).map(|_| rng.gen_prime(lambda)).collect::<Vec<_>>();

            for x in &xs {
                acc.add(x);
            }

            let ws = xs
                .iter()
                .map(|x| {
                    let w = acc.mem_wit_create(x);
                    assert!(acc.ver_mem(&w, x));
                    w
                })
                .collect::<Vec<_>>();

            for (x, w) in xs.iter().zip(ws.iter()) {
                // remove x
                acc.del(x).unwrap();
                // make sure test now fails
                assert!(!acc.ver_mem(w, x));
            }
        }
    }

    #[test]
    fn test_universal() {
        let rng = &mut XorShiftRng::from_seed([0u8; 16]);

        for _ in 0..20 {
            let lambda = 256; // insecure, but faster tests
            let mut acc = RsaAccumulator::setup(rng, lambda);

            let xs = (0..5).map(|_| rng.gen_prime(lambda)).collect::<Vec<_>>();

            for x in &xs {
                acc.add(x);
            }

            for _ in 0..5 {
                let y = rng.gen_prime(lambda);

                let w = acc.non_mem_wit_create(&y);
                assert!(acc.ver_non_mem(&w, &y));
            }
        }
    }

    #[test]
    fn test_math_non_mempership() {
        let rng = &mut XorShiftRng::from_seed([0u8; 16]);

        let lambda = 32;

        let x = rng.gen_prime(lambda);
        let s1 = rng.gen_prime(lambda);
        let s2 = rng.gen_prime(lambda);

        let n = BigUint::from_u32(43 * 67).unwrap();
        let g = BigUint::from_u32(49).unwrap();

        // s* = \prod s
        let mut s_star = BigUint::one();
        s_star *= &s1;
        s_star *= &s2;

        // A = g ^ s*
        let a_t = g.modpow(&s_star, &n);

        let (_, a, b) = extended_gcd(&x, &s_star);
        println!("{} {} {} {}", &g, &a, &b, &n);

        let u = BigInt::from_biguint(Sign::Plus, x.clone());
        let v = BigInt::from_biguint(Sign::Plus, s_star);
        let lhs = a.clone() * &u;
        let rhs = b.clone() * &v;
        println!("> {} * {} + {} * {} == 1", &a, &u, &b, &v);
        assert_eq!(lhs + &rhs, BigInt::one());

        // d = g^a mod n
        let d = modpow_uint_int(&g, &a, &n).unwrap();
        println!("> {} = {}^{} mod {}", &d, &g, &a, &n);

        // A^b
        let a_b = modpow_uint_int(&a_t, &b, &n).unwrap();
        println!("> {} = {}^{} mod {}", &a_b, &a_t, &b, &n);

        // A^b == g^{s* * b}
        let res = modpow_uint_int(&g, &(&v * &b), &n).unwrap();
        println!("> {} = {}^({} * {}) mod {}", &res, &g, &v, &b, &n);
        assert_eq!(a_b, res);

        // d^x
        let d_x = d.modpow(&x, &n);
        println!("> (d_x) {} = {}^{} mod {}", &d_x, &d, &x, &n);

        // d^x == g^{a * x}
        let res = modpow_uint_int(&g, &(&a * &u), &n).unwrap();
        println!("> (d_x) {} = {}^({} * {}) mod {}", &res, &g, &a, &u, &n);
        assert_eq!(d_x, res);

        // d^x A^b == g
        let lhs = (&d_x * &a_b) % &n;
        println!("> {} = {} * {} mod {}", &lhs, &d_x, &a_b, &n);
        assert_eq!(lhs, g);
    }

    fn test_batch_add_size(size: usize) {
        println!("batch_add_size {}", size);
        let rng = &mut XorShiftRng::from_seed([0u8; 16]);

        let lambda = 256; // insecure, but faster tests
        let mut acc = RsaAccumulator::setup(rng, lambda);

        // regular add
        let x0 = rng.gen_prime(lambda);
        acc.add(&x0);

        // batch add
        let a_t = acc.state().clone();
        let xs = (0..size).map(|_| rng.gen_prime(lambda)).collect::<Vec<_>>();
        let w = acc.batch_add(&xs);

        // verify batch add
        assert!(acc.ver_batch_add(&w, &a_t, &xs), "ver_batch_add failed");

        // delete with member
        let x = &xs[2];
        let w = acc.mem_wit_create(x);
        assert!(acc.ver_mem(&w, x), "failed to verify valid witness");

        acc.del_w_mem(&w, x).unwrap();
        assert!(
            !acc.ver_mem(&w, x),
            "witness verified, even though it was deleted"
        );

        // create all members witness
        // current state contains xs\x + x0
        let mut s = vec![x0.clone(), xs[0].clone(), xs[1].clone()];
        s.extend(xs.iter().skip(3).cloned());

        let ws = acc.create_all_mem_wit(&s);

        for (w, x) in ws.iter().zip(s.iter()) {
            assert!(acc.ver_mem(w, x));
        }

        // batch delete
        let a_t = acc.state().clone();
        let pairs = s
            .iter()
            .cloned()
            .zip(ws.iter().cloned())
            .take(3)
            .collect::<Vec<_>>();
        let w = acc.batch_del(&pairs[..]).unwrap();

        assert!(acc.ver_batch_del(&w, &a_t, &s[..3]), "ver_batch_del failed");
    }

    #[test]
    fn test_batch_add_small() {
        for i in 4..14 {
            test_batch_add_size(i)
        }
    }

    #[test]
    fn test_batch_add_large() {
        let size = 128;
        let rng = &mut XorShiftRng::from_seed([0u8; 16]);
        let lambda = 256; // insecure, but faster tests
        let mut acc = RsaAccumulator::setup(rng, lambda);

        // regular add
        let x0 = rng.gen_prime(lambda);
        acc.add(&x0);

        // batch add
        let a_t = acc.state().clone();
        let xs = (0..size).map(|_| rng.gen_prime(lambda)).collect::<Vec<_>>();
        let w = acc.batch_add(&xs);

        // verify batch add
        assert!(acc.ver_batch_add(&w, &a_t, &xs), "ver_batch_add failed");

        // batch add
        let a_t = acc.state().clone();
        let xs = (0..size).map(|_| rng.gen_prime(lambda)).collect::<Vec<_>>();
        let w = acc.batch_add(&xs);

        // verify batch add
        assert!(acc.ver_batch_add(&w, &a_t, &xs), "ver_batch_add failed");
    }

    #[test]
    fn test_aggregation() {
        let rng = &mut XorShiftRng::from_seed([0u8; 16]);

        for _ in 0..10 {
            let lambda = 256; // insecure, but faster tests
            let mut acc = RsaAccumulator::setup(rng, lambda);

            // regular add
            let xs = (0..5).map(|_| rng.gen_prime(lambda)).collect::<Vec<_>>();

            for x in &xs {
                acc.add(x);
            }

            // AggMemWit
            {
                let x = &xs[0];
                let y = &xs[1];
                let w_x = acc.mem_wit_create(x);
                let w_y = acc.mem_wit_create(y);

                let (w_xy, p_wxy) = acc.agg_mem_wit(&w_x, &w_y, x, y);

                assert!(
                    acc.ver_agg_mem_wit(&w_xy, &p_wxy, x, y),
                    "invalid agg_mem_wit proof"
                );
            }

            // MemWitCreate*
            {
                let pis = (0..5)
                    .map(|i| acc.mem_wit_create_star(&xs[i]))
                    .collect::<Vec<_>>();
                for (pi, x) in pis.iter().zip(&xs) {
                    assert!(acc.ver_mem_star(x, pi), "invalid mem_wit_create_star proof");
                }
            }

            // MemWitX
            {
                let mut acc = RsaAccumulator::setup(rng, lambda);
                let mut other = acc.clone();
                let x = rng.gen_prime(128);
                let y = rng.gen_prime(128);

                assert!(extended_gcd(&x, &y).0.is_one(), "x, y must be coprime");

                acc.add(&x);
                other.add(&y);

                let w_x = acc.mem_wit_create(&x);
                let w_y = other.mem_wit_create(&y);

                assert!(acc.ver_mem(&w_x, &x));
                assert!(other.ver_mem(&w_y, &y));

                let w_xy = acc.mem_wit_x(other.state(), &w_x, &w_y, &x, &y);
                assert!(
                    acc.ver_mem_x(other.state(), &w_xy, &x, &y),
                    "invalid ver_mem_x witness"
                );
            }
        }
    }

    #[test]
    fn test_aggregation_non_mem_star() {
        let rng = &mut XorShiftRng::from_seed([0u8; 16]);

        for _ in 0..10 {
            let lambda = 256; // insecure, but faster tests
            let mut acc = RsaAccumulator::setup(rng, lambda);

            // regular add
            let xs = (0..5).map(|_| rng.gen_prime(lambda)).collect::<Vec<_>>();

            for x in &xs {
                acc.add(x);
            }

            let x = rng.gen_prime(lambda);
            let pi = acc.non_mem_wit_create_star(&x);

            assert!(acc.ver_non_mem_star(&x, &pi), "invalid ver_non_mem_star");
        }
    }
}
