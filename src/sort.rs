//! `list.sort` of the baseline interpreter (listobject.c of release 3.12): run
//! detection, binary insertion up to the minimum run, the powersort merge
//! policy and galloping merges with the adaptive `min_gallop`.
//!
//! The comparison may be inconsistent (a NaN key) or fail (keys of types that
//! do not order). The baseline's result then depends on which pairs it
//! compared, so this follows its steps one for one: the same comparisons in the
//! same order give the same final order, and the first failing comparison ends
//! the sort with its error.

use crate::error::Res;

const MIN_GALLOP: usize = 7;

/// Where a run of keys lies: the list being sorted or the merge buffer.
#[derive(Clone, Copy)]
enum Area {
    List,
    Temp,
}

#[derive(Clone, Copy)]
struct Run {
    base: usize,
    len: usize,
    power: u32,
}

struct State<'a, T, F> {
    items: &'a [T],
    lt: F,
    /// The permutation being sorted: `keys[i]` is the index of an item.
    keys: Vec<usize>,
    temp: Vec<usize>,
    min_gallop: usize,
    pending: Vec<Run>,
}

/// Sort `items` as the baseline's `list.sort` did, `lt` being its `<` on two
/// items. On an error the items are left as they were.
pub fn list_sort<T>(items: &mut [T], lt: impl FnMut(&T, &T) -> Res<bool>) -> Res<()> {
    let n = items.len();
    if n < 2 {
        return Ok(());
    }
    let order = {
        let mut state = State {
            items,
            lt,
            keys: (0..n).collect(),
            temp: Vec::new(),
            min_gallop: MIN_GALLOP,
            pending: Vec::new(),
        };
        state.sort()?;
        state.keys
    };
    permute(items, &order);
    Ok(())
}

/// Reorder `items` so that position `i` holds the item that was at `order[i]`.
fn permute<T>(items: &mut [T], order: &[usize]) {
    for i in 0..order.len() {
        let mut from = order[i];
        while from < i {
            from = order[from];
        }
        items.swap(i, from);
    }
}

/// The minimum run length for `n` items.
fn min_run(mut n: usize) -> usize {
    let mut r = 0;
    while n >= 64 {
        r |= n & 1;
        n >>= 1;
    }
    n + r
}

/// The powersort node power of two adjacent runs `[s1, s1+n1)` and `[s1+n1, s1+n1+n2)`
/// in a list of `n` items.
fn power_of(s1: usize, n1: usize, n2: usize, n: usize) -> u32 {
    let mut result = 0;
    let mut a = 2 * s1 + n1;
    let mut b = a + n1 + n2;
    loop {
        result += 1;
        if a >= n {
            a -= n;
            b -= n;
        } else if b >= n {
            break;
        }
        a <<= 1;
        b <<= 1;
    }
    result
}

impl<T, F: FnMut(&T, &T) -> Res<bool>> State<'_, T, F> {
    /// `x < y` for two item indices.
    fn less(&mut self, x: usize, y: usize) -> Res<bool> {
        (self.lt)(&self.items[x], &self.items[y])
    }

    fn at(&self, area: Area, i: usize) -> usize {
        match area {
            Area::List => self.keys[i],
            Area::Temp => self.temp[i],
        }
    }

    fn sort(&mut self) -> Res<()> {
        let total = self.keys.len();
        let minrun = min_run(total);
        let mut lo = 0;
        let mut remaining = total;
        while remaining > 0 {
            let (mut n, descending) = self.count_run(lo, lo + remaining)?;
            if descending {
                self.keys[lo..lo + n].reverse();
            }
            if n < minrun {
                let force = remaining.min(minrun);
                self.binary_sort(lo, lo + force, lo + n)?;
                n = force;
            }
            self.found_new_run(n)?;
            self.pending.push(Run {
                base: lo,
                len: n,
                power: 0,
            });
            lo += n;
            remaining -= n;
        }
        self.force_collapse()
    }

    /// Length of the run at `lo` and whether it is strictly descending.
    fn count_run(&mut self, lo: usize, hi: usize) -> Res<(usize, bool)> {
        if lo + 1 == hi {
            return Ok((1, false));
        }
        let mut n = 2;
        if self.less(self.keys[lo + 1], self.keys[lo])? {
            let mut p = lo + 2;
            while p < hi && self.less(self.keys[p], self.keys[p - 1])? {
                p += 1;
                n += 1;
            }
            Ok((n, true))
        } else {
            let mut p = lo + 2;
            while p < hi && !self.less(self.keys[p], self.keys[p - 1])? {
                p += 1;
                n += 1;
            }
            Ok((n, false))
        }
    }

    /// Binary insertion of `keys[start..hi]` into the sorted `keys[lo..start]`.
    fn binary_sort(&mut self, lo: usize, hi: usize, mut start: usize) -> Res<()> {
        if lo == start {
            start += 1;
        }
        while start < hi {
            let pivot = self.keys[start];
            let (mut l, mut r) = (lo, start);
            loop {
                let p = l + ((r - l) >> 1);
                if self.less(pivot, self.keys[p])? {
                    r = p;
                } else {
                    l = p + 1;
                }
                if l >= r {
                    break;
                }
            }
            self.keys[l..=start].rotate_right(1);
            start += 1;
        }
        Ok(())
    }

    fn found_new_run(&mut self, n2: usize) -> Res<()> {
        if let Some(last) = self.pending.last().copied() {
            let power = power_of(last.base, last.len, n2, self.keys.len());
            while self.pending.len() > 1 && self.pending[self.pending.len() - 2].power > power {
                self.merge_at(self.pending.len() - 2)?;
            }
            let top = self.pending.len() - 1;
            self.pending[top].power = power;
        }
        Ok(())
    }

    fn force_collapse(&mut self) -> Res<()> {
        while self.pending.len() > 1 {
            let mut n = self.pending.len() - 2;
            if n > 0 && self.pending[n - 1].len < self.pending[n + 1].len {
                n -= 1;
            }
            self.merge_at(n)?;
        }
        Ok(())
    }

    /// Merge pending runs `i` and `i + 1`.
    fn merge_at(&mut self, i: usize) -> Res<()> {
        let Run {
            base: mut base_a,
            len: mut na,
            ..
        } = self.pending[i];
        let Run {
            base: base_b,
            len: nb,
            ..
        } = self.pending[i + 1];
        self.pending[i].len = na + nb;
        self.pending.remove(i + 1);
        let k = self.gallop_right(self.keys[base_b], Area::List, base_a, na, 0)?;
        base_a += k;
        na -= k;
        if na == 0 {
            return Ok(());
        }
        let nb = self.gallop_left(self.keys[base_a + na - 1], Area::List, base_b, nb, nb - 1)?;
        if nb == 0 {
            return Ok(());
        }
        if na <= nb {
            self.merge_lo(base_a, na, base_b, nb)
        } else {
            self.merge_hi(base_a, na, base_b, nb)
        }
    }

    /// The leftmost place for `key` in the sorted `n` keys at `start` of `area`, starting at `hint`.
    fn gallop_left(
        &mut self,
        key: usize,
        area: Area,
        start: usize,
        n: usize,
        hint: usize,
    ) -> Res<usize> {
        let a = |s: &Self, i: isize| s.at(area, (start as isize + i) as usize);
        let hint = hint as isize;
        let n = n as isize;
        let mut lastofs: isize = 0;
        let mut ofs: isize = 1;
        let ah = a(self, hint);
        if self.less(ah, key)? {
            let maxofs = n - hint;
            while ofs < maxofs {
                let x = a(self, hint + ofs);
                if self.less(x, key)? {
                    lastofs = ofs;
                    ofs = (ofs << 1) + 1;
                } else {
                    break;
                }
            }
            ofs = ofs.min(maxofs);
            lastofs += hint;
            ofs += hint;
        } else {
            let maxofs = hint + 1;
            while ofs < maxofs {
                let x = a(self, hint - ofs);
                if self.less(x, key)? {
                    break;
                }
                lastofs = ofs;
                ofs = (ofs << 1) + 1;
            }
            ofs = ofs.min(maxofs);
            let k = lastofs;
            lastofs = hint - ofs;
            ofs = hint - k;
        }
        lastofs += 1;
        while lastofs < ofs {
            let m = lastofs + ((ofs - lastofs) >> 1);
            let x = a(self, m);
            if self.less(x, key)? {
                lastofs = m + 1;
            } else {
                ofs = m;
            }
        }
        Ok(ofs as usize)
    }

    /// The rightmost place for `key` in the sorted `n` keys at `start` of `area`, starting at `hint`.
    fn gallop_right(
        &mut self,
        key: usize,
        area: Area,
        start: usize,
        n: usize,
        hint: usize,
    ) -> Res<usize> {
        let a = |s: &Self, i: isize| s.at(area, (start as isize + i) as usize);
        let hint = hint as isize;
        let n = n as isize;
        let mut lastofs: isize = 0;
        let mut ofs: isize = 1;
        let ah = a(self, hint);
        if self.less(key, ah)? {
            let maxofs = hint + 1;
            while ofs < maxofs {
                let x = a(self, hint - ofs);
                if self.less(key, x)? {
                    lastofs = ofs;
                    ofs = (ofs << 1) + 1;
                } else {
                    break;
                }
            }
            ofs = ofs.min(maxofs);
            let k = lastofs;
            lastofs = hint - ofs;
            ofs = hint - k;
        } else {
            let maxofs = n - hint;
            while ofs < maxofs {
                let x = a(self, hint + ofs);
                if self.less(key, x)? {
                    break;
                }
                lastofs = ofs;
                ofs = (ofs << 1) + 1;
            }
            ofs = ofs.min(maxofs);
            lastofs += hint;
            ofs += hint;
        }
        lastofs += 1;
        while lastofs < ofs {
            let m = lastofs + ((ofs - lastofs) >> 1);
            let x = a(self, m);
            if self.less(key, x)? {
                ofs = m;
            } else {
                lastofs = m + 1;
            }
        }
        Ok(ofs as usize)
    }

    /// Merge the adjacent runs `[base_a, +na)` and `[base_b, +nb)` with `na <= nb`,
    /// the first run moved to the buffer.
    fn merge_lo(&mut self, base_a: usize, mut na: usize, base_b: usize, mut nb: usize) -> Res<()> {
        self.temp.clear();
        self.temp.extend_from_slice(&self.keys[base_a..base_a + na]);
        let mut dest = base_a;
        let mut pa = 0; // in temp
        let mut pb = base_b; // in keys
        let result = (|| -> Res<bool> {
            // Ok(true): copy B then the last A; Ok(false): done, rest of A goes last.
            self.keys[dest] = self.keys[pb];
            dest += 1;
            pb += 1;
            nb -= 1;
            if nb == 0 {
                return Ok(false);
            }
            if na == 1 {
                return Ok(true);
            }
            let mut min_gallop = self.min_gallop;
            loop {
                let mut acount = 0;
                let mut bcount = 0;
                loop {
                    if self.less(self.keys[pb], self.temp[pa])? {
                        self.keys[dest] = self.keys[pb];
                        dest += 1;
                        pb += 1;
                        bcount += 1;
                        acount = 0;
                        nb -= 1;
                        if nb == 0 {
                            return Ok(false);
                        }
                        if bcount >= min_gallop {
                            break;
                        }
                    } else {
                        self.keys[dest] = self.temp[pa];
                        dest += 1;
                        pa += 1;
                        acount += 1;
                        bcount = 0;
                        na -= 1;
                        if na == 1 {
                            return Ok(true);
                        }
                        if acount >= min_gallop {
                            break;
                        }
                    }
                }
                min_gallop += 1;
                loop {
                    min_gallop -= usize::from(min_gallop > 1);
                    self.min_gallop = min_gallop;
                    let k = self.gallop_right(self.keys[pb], Area::Temp, pa, na, 0)?;
                    acount = k;
                    if k > 0 {
                        self.keys[dest..dest + k].copy_from_slice(&self.temp[pa..pa + k]);
                        dest += k;
                        pa += k;
                        na -= k;
                        if na == 1 {
                            return Ok(true);
                        }
                        if na == 0 {
                            return Ok(false);
                        }
                    }
                    self.keys[dest] = self.keys[pb];
                    dest += 1;
                    pb += 1;
                    nb -= 1;
                    if nb == 0 {
                        return Ok(false);
                    }
                    let k = self.gallop_left(self.temp[pa], Area::List, pb, nb, 0)?;
                    bcount = k;
                    if k > 0 {
                        self.keys.copy_within(pb..pb + k, dest);
                        dest += k;
                        pb += k;
                        nb -= k;
                        if nb == 0 {
                            return Ok(false);
                        }
                    }
                    self.keys[dest] = self.temp[pa];
                    dest += 1;
                    pa += 1;
                    na -= 1;
                    if na == 1 {
                        return Ok(true);
                    }
                    if acount < MIN_GALLOP && bcount < MIN_GALLOP {
                        break;
                    }
                }
                min_gallop += 1;
                self.min_gallop = min_gallop;
            }
        })()?;
        if result {
            self.keys.copy_within(pb..pb + nb, dest);
            self.keys[dest + nb] = self.temp[pa];
        } else if na > 0 {
            self.keys[dest..dest + na].copy_from_slice(&self.temp[pa..pa + na]);
        }
        Ok(())
    }

    /// Merge the adjacent runs `[base_a, +na)` and `[base_b, +nb)` with `na > nb`,
    /// from the right, the second run moved to the buffer.
    fn merge_hi(&mut self, base_a: usize, mut na: usize, base_b: usize, mut nb: usize) -> Res<()> {
        self.temp.clear();
        self.temp.extend_from_slice(&self.keys[base_b..base_b + nb]);
        let base_a = base_a as isize;
        let mut dest = (base_b + nb - 1) as isize; // in keys
        let mut pa = base_a + na as isize - 1; // in keys
        let mut pb = nb as isize - 1; // in temp
        let ix = |i: isize| i as usize;
        let result = (|| -> Res<bool> {
            // Ok(true): copy the rest of A then the first B; Ok(false): done, rest of B goes first.
            self.keys[ix(dest)] = self.keys[ix(pa)];
            dest -= 1;
            pa -= 1;
            na -= 1;
            if na == 0 {
                return Ok(false);
            }
            if nb == 1 {
                return Ok(true);
            }
            let mut min_gallop = self.min_gallop;
            loop {
                let mut acount = 0;
                let mut bcount = 0;
                loop {
                    if self.less(self.temp[ix(pb)], self.keys[ix(pa)])? {
                        self.keys[ix(dest)] = self.keys[ix(pa)];
                        dest -= 1;
                        pa -= 1;
                        acount += 1;
                        bcount = 0;
                        na -= 1;
                        if na == 0 {
                            return Ok(false);
                        }
                        if acount >= min_gallop {
                            break;
                        }
                    } else {
                        self.keys[ix(dest)] = self.temp[ix(pb)];
                        dest -= 1;
                        pb -= 1;
                        bcount += 1;
                        acount = 0;
                        nb -= 1;
                        if nb == 1 {
                            return Ok(true);
                        }
                        if bcount >= min_gallop {
                            break;
                        }
                    }
                }
                min_gallop += 1;
                loop {
                    min_gallop -= usize::from(min_gallop > 1);
                    self.min_gallop = min_gallop;
                    let found =
                        self.gallop_right(self.temp[ix(pb)], Area::List, ix(base_a), na, na - 1)?;
                    let k = na - found;
                    acount = k;
                    if k > 0 {
                        let ki = k as isize;
                        dest -= ki;
                        pa -= ki;
                        self.keys
                            .copy_within(ix(pa + 1)..ix(pa + 1 + ki), ix(dest + 1));
                        na -= k;
                        if na == 0 {
                            return Ok(false);
                        }
                    }
                    self.keys[ix(dest)] = self.temp[ix(pb)];
                    dest -= 1;
                    pb -= 1;
                    nb -= 1;
                    if nb == 1 {
                        return Ok(true);
                    }
                    let found = self.gallop_left(self.keys[ix(pa)], Area::Temp, 0, nb, nb - 1)?;
                    let k = nb - found;
                    bcount = k;
                    if k > 0 {
                        let ki = k as isize;
                        dest -= ki;
                        pb -= ki;
                        let (from, to) = (ix(pb + 1), ix(dest + 1));
                        self.keys[to..to + k].copy_from_slice(&self.temp[from..from + k]);
                        nb -= k;
                        if nb == 1 {
                            return Ok(true);
                        }
                        if nb == 0 {
                            return Ok(false);
                        }
                    }
                    self.keys[ix(dest)] = self.keys[ix(pa)];
                    dest -= 1;
                    pa -= 1;
                    na -= 1;
                    if na == 0 {
                        return Ok(false);
                    }
                    if acount < MIN_GALLOP && bcount < MIN_GALLOP {
                        break;
                    }
                }
                min_gallop += 1;
                self.min_gallop = min_gallop;
            }
        })()?;
        if result {
            let nai = na as isize;
            self.keys
                .copy_within(ix(pa + 1 - nai)..ix(pa + 1), ix(dest + 1 - nai));
            dest -= nai;
            self.keys[ix(dest)] = self.temp[ix(pb)];
        } else if nb > 0 {
            let to = ix(dest + 1 - nb as isize);
            self.keys[to..to + nb].copy_from_slice(&self.temp[..nb]);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sorted_floats(values: &[f64]) -> Vec<f64> {
        let mut v = values.to_vec();
        list_sort(&mut v, |a, b| Ok(a < b)).unwrap();
        v
    }

    #[test]
    fn consistent_orders_match_a_stable_sort() {
        let mut seed = 12345u64;
        for n in [0, 1, 2, 5, 63, 64, 65, 100, 257, 1000] {
            let values: Vec<(u32, usize)> = (0..n)
                .map(|i| {
                    seed = seed
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    (((seed >> 33) % 17) as u32, i)
                })
                .collect();
            let mut mine = values.clone();
            list_sort(&mut mine, |a, b| Ok(a.0 < b.0)).unwrap();
            let mut expected = values;
            expected.sort_by_key(|v| v.0);
            assert_eq!(mine, expected);
        }
    }

    #[test]
    fn nan_keys_give_the_baseline_order() {
        // the baseline: x = [float(i) for i in range(70)]; x[5] = nan; sorted(x[::-1])
        let mut values: Vec<f64> = (0..70).map(f64::from).collect();
        values[5] = f64::NAN;
        values.reverse();
        let out = sorted_floats(&values);
        // the baseline's answer: 6.0 ..= 69.0, nan, 0.0 ..= 4.0
        let mut want: Vec<f64> = (6..70).map(f64::from).collect();
        want.push(f64::NAN);
        want.extend((0..5).map(f64::from));
        let same = |a: &f64, b: &f64| a == b || (a.is_nan() && b.is_nan());
        assert!(out.len() == want.len() && out.iter().zip(&want).all(|(a, b)| same(a, b)));
    }

    #[test]
    fn a_failing_comparison_stops_the_sort() {
        let mut v: Vec<i32> = (0..100).rev().collect();
        let err = list_sort(&mut v, |a, b| {
            if *a == 50 || *b == 50 {
                Err(crate::error::PmError::Internal("TypeError: x".into()))
            } else {
                Ok(a < b)
            }
        });
        assert!(err.is_err());
    }
}
