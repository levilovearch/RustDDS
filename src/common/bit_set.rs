use bit_set::BitSet;
use bit_vec::BitVec;
use speedy::{Context, Readable, Reader, Writable, Writer};
use std::ops::{Deref, DerefMut};

#[derive(Debug, PartialEq)]
pub struct BitSetRef(BitSet);

impl BitSetRef {
  pub fn new() -> BitSetRef {
    BitSetRef(BitSet::with_capacity(0))
  }

  pub fn into_bit_set(self) -> BitSet {
    self.0
  }
}

impl Deref for BitSetRef {
  type Target = BitSet;

  fn deref(&self) -> &BitSet {
    &self.0
  }
}

impl DerefMut for BitSetRef {
  fn deref_mut(&mut self) -> &mut BitSet {
    &mut self.0
  }
}

impl<'a, C: Context> Readable<'a, C> for BitSetRef {
  #[inline]
  fn read_from<R: Reader<'a, C>>(reader: &mut R) -> Result<Self, C::Error> {
    let number_of_bits = reader.read_u32()?;
    let mut bit_vec = BitVec::with_capacity(number_of_bits as usize);
    unsafe {
      let inner = bit_vec.storage_mut();
      for _ in 0..(number_of_bits / 32) {
        inner.push(reader.read_u32()?);
      }
    }
    Ok(BitSetRef(BitSet::from_bit_vec(bit_vec)))
  }

  #[inline]
  fn minimum_bytes_needed() -> usize {
    4
  }
}

impl<C: Context> Writable<C> for BitSetRef {
  #[inline]
  fn write_to<T: ?Sized + Writer<C>>(&self, writer: &mut T) -> Result<(), C::Error> {
    let bytes = self.get_ref().storage();
    let number_of_bytes = bytes.len() as u32 * 32;
    writer.write_u32(number_of_bytes)?;
    for byte in bytes {
      let lz = byte.leading_zeros();
      let foo = byte.rotate_left(lz);
      writer.write_u32(foo)?;
    }
    Ok(())
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  serialization_test!( type = BitSetRef,
  {
      bit_set_empty,
      BitSetRef::new(),
      le = [0x00, 0x00, 0x00, 0x00],
      be = [0x00, 0x00, 0x00, 0x00]
  },
  {
      bit_set_non_zero_size,
      (|| {
          let mut set = BitSetRef::new();
          set.insert(0);
          set.insert(42);
          set.insert(7);
          set
      })(),
      le = [0x40, 0x00, 0x00, 0x00,
            0x81, 0x00, 0x00, 0x00,
            0x00, 0x04, 0x00, 0x00],
      be = [0x00, 0x00, 0x00, 0x40,
            0x00, 0x00, 0x00, 0x81,
            0x00, 0x00, 0x04, 0x00]
  });
}
