// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! The logical owned bytes of the reader's record, counted through its own `Serialize`: every
//! number at its width, every string and sequence at its length plus a vector's header, every
//! option and enum at a tag. The record is plain owned data (vectors of numbers, nested enums),
//! so this counts what it holds without a hand-kept walk over each of its variants.

use serde::ser::{self, Serialize};

/// The record's logical owned bytes.
pub(crate) fn of<T: Serialize + ?Sized>(value: &T) -> usize {
    let mut counter = Counter(0);
    // The counter never fails; a value that refuses to serialize is counted as far as it got.
    let _ = value.serialize(&mut counter);
    counter.0
}

const HEADER: usize = size_of::<Vec<u8>>();
const TAG: usize = size_of::<u64>();

struct Counter(usize);

#[derive(Debug)]
struct Never;
impl std::fmt::Display for Never {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("record byte counter")
    }
}
impl std::error::Error for Never {}
impl ser::Error for Never {
    fn custom<T: std::fmt::Display>(_: T) -> Self {
        Never
    }
}

impl Counter {
    fn add(&mut self, bytes: usize) {
        self.0 = self.0.saturating_add(bytes);
    }
}

impl ser::Serializer for &mut Counter {
    type Ok = ();
    type Error = Never;
    type SerializeSeq = Self;
    type SerializeTuple = Self;
    type SerializeTupleStruct = Self;
    type SerializeTupleVariant = Self;
    type SerializeMap = Self;
    type SerializeStruct = Self;
    type SerializeStructVariant = Self;

    fn serialize_bool(self, _: bool) -> Result<(), Never> {
        self.add(size_of::<bool>());
        Ok(())
    }
    fn serialize_i8(self, _: i8) -> Result<(), Never> {
        self.add(size_of::<i8>());
        Ok(())
    }
    fn serialize_i16(self, _: i16) -> Result<(), Never> {
        self.add(size_of::<i16>());
        Ok(())
    }
    fn serialize_i32(self, _: i32) -> Result<(), Never> {
        self.add(size_of::<i32>());
        Ok(())
    }
    fn serialize_i64(self, _: i64) -> Result<(), Never> {
        self.add(size_of::<i64>());
        Ok(())
    }
    fn serialize_u8(self, _: u8) -> Result<(), Never> {
        self.add(size_of::<u8>());
        Ok(())
    }
    fn serialize_u16(self, _: u16) -> Result<(), Never> {
        self.add(size_of::<u16>());
        Ok(())
    }
    fn serialize_u32(self, _: u32) -> Result<(), Never> {
        self.add(size_of::<u32>());
        Ok(())
    }
    fn serialize_u64(self, _: u64) -> Result<(), Never> {
        self.add(size_of::<u64>());
        Ok(())
    }
    fn serialize_f32(self, _: f32) -> Result<(), Never> {
        self.add(size_of::<f32>());
        Ok(())
    }
    fn serialize_f64(self, _: f64) -> Result<(), Never> {
        self.add(size_of::<f64>());
        Ok(())
    }
    fn serialize_char(self, _: char) -> Result<(), Never> {
        self.add(size_of::<char>());
        Ok(())
    }
    fn serialize_str(self, value: &str) -> Result<(), Never> {
        self.add(HEADER.saturating_add(value.len()));
        Ok(())
    }
    fn serialize_bytes(self, value: &[u8]) -> Result<(), Never> {
        self.add(HEADER.saturating_add(value.len()));
        Ok(())
    }
    fn serialize_none(self) -> Result<(), Never> {
        self.add(TAG);
        Ok(())
    }
    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<(), Never> {
        self.add(TAG);
        value.serialize(self)
    }
    fn serialize_unit(self) -> Result<(), Never> {
        Ok(())
    }
    fn serialize_unit_struct(self, _: &'static str) -> Result<(), Never> {
        Ok(())
    }
    fn serialize_unit_variant(self, _: &'static str, _: u32, _: &'static str) -> Result<(), Never> {
        self.add(TAG);
        Ok(())
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        value: &T,
    ) -> Result<(), Never> {
        value.serialize(self)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        value: &T,
    ) -> Result<(), Never> {
        self.add(TAG);
        value.serialize(self)
    }
    fn serialize_seq(self, _: Option<usize>) -> Result<Self, Never> {
        self.add(HEADER);
        Ok(self)
    }
    fn serialize_tuple(self, _: usize) -> Result<Self, Never> {
        Ok(self)
    }
    fn serialize_tuple_struct(self, _: &'static str, _: usize) -> Result<Self, Never> {
        Ok(self)
    }
    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self, Never> {
        self.add(TAG);
        Ok(self)
    }
    fn serialize_map(self, _: Option<usize>) -> Result<Self, Never> {
        self.add(HEADER);
        Ok(self)
    }
    fn serialize_struct(self, _: &'static str, _: usize) -> Result<Self, Never> {
        Ok(self)
    }
    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self, Never> {
        self.add(TAG);
        Ok(self)
    }
}

impl ser::SerializeSeq for &mut Counter {
    type Ok = ();
    type Error = Never;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Never> {
        value.serialize(&mut **self)
    }
    fn end(self) -> Result<(), Never> {
        Ok(())
    }
}
impl ser::SerializeTuple for &mut Counter {
    type Ok = ();
    type Error = Never;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Never> {
        value.serialize(&mut **self)
    }
    fn end(self) -> Result<(), Never> {
        Ok(())
    }
}
impl ser::SerializeTupleStruct for &mut Counter {
    type Ok = ();
    type Error = Never;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Never> {
        value.serialize(&mut **self)
    }
    fn end(self) -> Result<(), Never> {
        Ok(())
    }
}
impl ser::SerializeTupleVariant for &mut Counter {
    type Ok = ();
    type Error = Never;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Never> {
        value.serialize(&mut **self)
    }
    fn end(self) -> Result<(), Never> {
        Ok(())
    }
}
impl ser::SerializeMap for &mut Counter {
    type Ok = ();
    type Error = Never;
    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), Never> {
        key.serialize(&mut **self)
    }
    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Never> {
        value.serialize(&mut **self)
    }
    fn end(self) -> Result<(), Never> {
        Ok(())
    }
}
impl ser::SerializeStruct for &mut Counter {
    type Ok = ();
    type Error = Never;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        _: &'static str,
        value: &T,
    ) -> Result<(), Never> {
        value.serialize(&mut **self)
    }
    fn end(self) -> Result<(), Never> {
        Ok(())
    }
}
impl ser::SerializeStructVariant for &mut Counter {
    type Ok = ();
    type Error = Never;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        _: &'static str,
        value: &T,
    ) -> Result<(), Never> {
        value.serialize(&mut **self)
    }
    fn end(self) -> Result<(), Never> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn counts_numbers_vectors_and_tags() {
        assert_eq!(super::of(&1.0_f64), 8);
        assert_eq!(super::of(&vec![1.0_f64, 2.0]), super::HEADER + 16);
        assert_eq!(super::of(&Some(3_u32)), super::TAG + 4);
        assert_eq!(super::of(&[[0.0_f64; 3]; 2]), 48);
    }
}
