//! Streaming literal credential redaction before output reaches logs or callbacks.
#[derive(Clone, Default)]
pub(crate) struct Redactor {
    secrets: Vec<Vec<u8>>,
    pending: Vec<u8>,
}
impl Redactor {
    pub(crate) fn new(mut secrets: Vec<Vec<u8>>) -> Self {
        secrets.retain(|s| !s.is_empty());
        secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
        Self {
            secrets,
            pending: Vec::new(),
        }
    }
    pub(crate) fn push(&mut self, bytes: &[u8], eof: bool) -> Vec<u8> {
        if self.secrets.is_empty() {
            return bytes.to_vec();
        }
        self.pending.extend_from_slice(bytes);
        let mut output = Vec::new();
        let mut offset = 0;
        while offset < self.pending.len() {
            let tail = &self.pending[offset..];
            if !eof
                && self
                    .secrets
                    .iter()
                    .any(|s| s.len() > tail.len() && s.starts_with(tail))
            {
                break;
            }
            if let Some(secret) = self.secrets.iter().find(|s| tail.starts_with(s)) {
                output.extend_from_slice(b"[REDACTED]");
                offset += secret.len();
            } else {
                output.push(tail[0]);
                offset += 1;
            }
        }
        self.pending.drain(..offset);
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn redacts_every_split_and_overlapping_prefix_without_delaying_plain_output() {
        for split in 0..=14 {
            let mut r = Redactor::new(vec![b"secret".to_vec(), b"secret-long".to_vec()]);
            let bytes = b"a secret-long!";
            let mut output = r.push(&bytes[..split], false);
            output.extend(r.push(&bytes[split..], true));
            assert_eq!(output, b"a [REDACTED]!");
        }
        assert_eq!(Redactor::default().push(b"hello", false), b"hello");
    }
}
