//! Host allocation accounting for the peak-memory preflight (FL-043; plan
//! §19.3, §19.4). A buffer is charged what its allocation holds, its
//! capacity times its element size, not what it currently uses.

/// The bytes `vector`'s allocation holds: its capacity times its element
/// size, saturating at `u64::MAX`.
#[must_use]
pub fn capacity_bytes<T>(vector: &Vec<T>) -> u64 {
    bytes_for::<T>(vector.capacity())
}

/// The bytes `count` values of `T` take, saturating at `u64::MAX`.
#[must_use]
pub const fn bytes_for<T>(count: usize) -> u64 {
    (count as u64).saturating_mul(size_of::<T>() as u64)
}

/// The sum of `terms`, saturating at `u64::MAX`.
#[must_use]
pub fn total(terms: impl IntoIterator<Item = u64>) -> u64 {
    terms.into_iter().fold(0, u64::saturating_add)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_vector_is_charged_its_capacity_not_its_length() {
        let mut vector: Vec<u64> = Vec::with_capacity(10);
        vector.push(1);
        assert_eq!(capacity_bytes(&vector), 80);
        assert_eq!(bytes_for::<[f32; 3]>(5), 60);
        assert_eq!(bytes_for::<u64>(usize::MAX), u64::MAX);
        assert_eq!(total([1, 2, u64::MAX]), u64::MAX);
        assert_eq!(total([1, 2, 3]), 6);
    }
}
