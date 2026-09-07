use flate2::{Compress, CompressError, FlushCompress};

/// The FragmentCompressor splits the given payload into fragmented compressed frames.
pub struct FragmentCompressor<'a> {
  total_in: usize,
  done: bool,
  payload: &'a [u8],
  compress: &'a mut Compress,
  /// bytes withheld because they might be part of the trailing marker
  pending_tail: Vec<u8>,
}

impl<'a> FragmentCompressor<'a> {
  const FRAGMENT_LENGTH: usize = 64 * 1024; // 64KB

  pub fn new(payload: &'a [u8], compress: &'a mut Compress) -> Self {
    Self {
      done: false,
      total_in: 0,
      payload,
      compress,
      pending_tail: Vec::new(),
    }
  }
}

impl<'a> Iterator for FragmentCompressor<'a> {
  type Item = Result<(bool, Vec<u8>), CompressError>;

  fn next(&mut self) -> Option<Self::Item> {
    if self.done {
      return None;
    }

    let mut output = vec![0; Self::FRAGMENT_LENGTH];

    let in_before = self.compress.total_in();
    let out_before = self.compress.total_out();

    if let Err(err) = self.compress.compress(
      &self.payload[self.total_in..],
      &mut output,
      FlushCompress::Sync,
    ) {
      return Some(Err(err));
    }

    let bytes_consumed = (self.compress.total_in() - in_before) as usize;
    let bytes_written = (self.compress.total_out() - out_before) as usize;

    self.total_in += bytes_consumed;
    output.truncate(bytes_written);

    // Combine anything withheld from last call with this call's fresh output
    self.pending_tail.extend_from_slice(&output);

    let all_input_consumed = self.total_in >= self.payload.len();
    let output_not_full = bytes_written < Self::FRAGMENT_LENGTH;
    self.done = all_input_consumed && output_not_full;

    if self.done {
      // Now we've truly seen the end: the full marker is guaranteed
      // to be present in pending_tail, safe to strip.
      let trimmed_len = self.pending_tail.len().saturating_sub(4);
      self.pending_tail.truncate(trimmed_len);
      let out = std::mem::take(&mut self.pending_tail);
      return Some(Ok((true, out)));
    }

    // Not done yet: only release bytes we're sure can't be part of the
    // eventual trailing marker — i.e. hold back the last 4 bytes always.
    if self.pending_tail.len() > 4 {
      let release_len = self.pending_tail.len() - 4;
      let out: Vec<u8> = self.pending_tail.drain(..release_len).collect();
      Some(Ok((false, out)))
    } else {
      // Nothing safe to release yet; emit empty continuation and keep pumping.
      Some(Ok((false, Vec::new())))
    }
  }
}
