// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Job-local exact operand binding, never a persisted identity or cache key.
//! Native Serialize derives include both implicit carriers/guide/boundaries,
//! complete spline controls/knots, and processor entity/pose/sense/range.
use super::*;
use serde::{Serialize, ser};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Binding([u8; 32]);

/// A same-function key, never a correspondence by itself. Only exact finite
/// parameter composition to literal native t may strip chart restrictions.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct FunctionBinding([u8; 32]);

#[derive(Debug)]
enum Error {
    Declined,
    Geometry(GeometryError),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("native operand binding unavailable")
    }
}
impl std::error::Error for Error {}
impl ser::Error for Error {
    fn custom<T: std::fmt::Display>(_message: T) -> Self { Self::Declined }
}

struct Hasher<'a> {
    hash: Sha256,
    work: &'a mut GeometryWorkBudget,
    cancel: &'a AtomicBool,
    depth: usize,
}
impl Hasher<'_> {
    fn bytes(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.work.charge(bytes.len().max(1) as u64, self.cancel)
            .map_err(Error::Geometry)?;
        self.hash.update(bytes);
        Ok(())
    }
    fn scalar(&mut self, tag: u8, bytes: &[u8]) -> Result<(), Error> {
        self.bytes(&[tag])?;
        self.bytes(bytes)
    }
    fn text(&mut self, text: &str) -> Result<(), Error> {
        self.bytes(&(text.len() as u64).to_le_bytes())?;
        self.bytes(text.as_bytes())
    }
    fn length(&mut self, length: usize) -> Result<(), Error> {
        self.bytes(&(length as u64).to_le_bytes())
    }
    fn begin(&mut self, tag: u8) -> Result<(), Error> {
        self.bytes(&[tag])?;
        if self.depth == MAX_DEPTH { return Err(Error::Declined); }
        self.depth += 1;
        Ok(())
    }
    fn end(&mut self, tag: u8) -> Result<(), Error> {
        self.bytes(&[tag])?;
        self.depth -= 1;
        Ok(())
    }
}
struct Compound<'a, 'b> {
    hasher: &'a mut Hasher<'b>,
    end: u8,
}
macro_rules! integer {
    ($method:ident, $ty:ty, $tag:expr) => {
        fn $method(self, value: $ty) -> Result<(), Error> {
            self.scalar($tag, &value.to_le_bytes())
        }
    };
}
impl<'a, 'b> ser::Serializer for &'a mut Hasher<'b> {
    type Ok = ();
    type Error = Error;
    type SerializeSeq = Compound<'a, 'b>;
    type SerializeTuple = Compound<'a, 'b>;
    type SerializeTupleStruct = Compound<'a, 'b>;
    type SerializeTupleVariant = Compound<'a, 'b>;
    type SerializeMap = ser::Impossible<(), Error>;
    type SerializeStruct = Compound<'a, 'b>;
    type SerializeStructVariant = Compound<'a, 'b>;
    fn serialize_bool(self, value: bool) -> Result<(), Error> {
        self.scalar(0, &[u8::from(value)])
    }
    integer!(serialize_i8, i8, 1);
    integer!(serialize_i16, i16, 2);
    integer!(serialize_i32, i32, 3);
    integer!(serialize_i64, i64, 4);
    integer!(serialize_i128, i128, 5);
    integer!(serialize_u8, u8, 6);
    integer!(serialize_u16, u16, 7);
    integer!(serialize_u32, u32, 8);
    integer!(serialize_u64, u64, 9);
    integer!(serialize_u128, u128, 10);
    fn serialize_f32(self, value: f32) -> Result<(), Error> {
        self.bytes(&[11])?;
        if !value.is_finite() { return Err(Error::Declined); }
        self.bytes(&(if value == 0.0 { 0 } else { value.to_bits() }).to_le_bytes())
    }
    fn serialize_f64(self, value: f64) -> Result<(), Error> {
        self.bytes(&[12])?;
        if !value.is_finite() { return Err(Error::Declined); }
        self.bytes(&(if value == 0.0 { 0 } else { value.to_bits() }).to_le_bytes())
    }
    fn serialize_char(self, value: char) -> Result<(), Error> {
        self.scalar(13, &(value as u32).to_le_bytes())
    }
    fn serialize_str(self, value: &str) -> Result<(), Error> {
        self.bytes(&[14])?;
        self.text(value)
    }
    fn serialize_bytes(self, value: &[u8]) -> Result<(), Error> {
        self.bytes(&[15])?;
        self.length(value.len())?;
        self.bytes(value)
    }
    fn serialize_none(self) -> Result<(), Error> { self.bytes(&[16]) }
    fn serialize_some<T: ?Sized + Serialize>(self, value: &T) -> Result<(), Error> {
        self.begin(17)?;
        value.serialize(&mut *self)?;
        self.end(145)
    }
    fn serialize_unit(self) -> Result<(), Error> { self.bytes(&[18]) }
    fn serialize_unit_struct(self, name: &'static str) -> Result<(), Error> {
        self.begin(19)?;
        self.text(name)?;
        self.end(147)
    }
    fn serialize_unit_variant(self, name: &'static str, index: u32, variant: &'static str) -> Result<(), Error> {
        self.begin(20)?;
        self.text(name)?;
        self.bytes(&index.to_le_bytes())?;
        self.text(variant)?;
        self.end(148)
    }
    fn serialize_newtype_struct<T: ?Sized + Serialize>(self, name: &'static str, value: &T) -> Result<(), Error> {
        self.begin(21)?;
        self.text(name)?;
        value.serialize(&mut *self)?;
        self.end(149)
    }
    fn serialize_newtype_variant<T: ?Sized + Serialize>(self, name: &'static str, index: u32, variant: &'static str, value: &T) -> Result<(), Error> {
        self.begin(22)?;
        self.text(name)?;
        self.bytes(&index.to_le_bytes())?;
        self.text(variant)?;
        value.serialize(&mut *self)?;
        self.end(150)
    }
    fn serialize_seq(self, length: Option<usize>) -> Result<Self::SerializeSeq, Error> {
        self.begin(23)?;
        match length {
            Some(length) => { self.bytes(&[1])?; self.length(length)?; }
            None => self.bytes(&[0])?,
        }
        Ok(Compound { hasher: self, end: 151 })
    }
    fn serialize_tuple(self, length: usize) -> Result<Self::SerializeTuple, Error> {
        self.begin(24)?;
        self.length(length)?;
        Ok(Compound { hasher: self, end: 152 })
    }
    fn serialize_tuple_struct(self, name: &'static str, length: usize) -> Result<Self::SerializeTupleStruct, Error> {
        self.begin(25)?;
        self.text(name)?;
        self.length(length)?;
        Ok(Compound { hasher: self, end: 153 })
    }
    fn serialize_tuple_variant(self, name: &'static str, index: u32, variant: &'static str, length: usize) -> Result<Self::SerializeTupleVariant, Error> {
        self.begin(26)?;
        self.text(name)?;
        self.bytes(&index.to_le_bytes())?;
        self.text(variant)?;
        self.length(length)?;
        Ok(Compound { hasher: self, end: 154 })
    }
    fn serialize_map(self, _length: Option<usize>) -> Result<Self::SerializeMap, Error> {
        self.bytes(&[255])?;
        Err(Error::Declined)
    }
    fn serialize_struct(self, name: &'static str, length: usize) -> Result<Self::SerializeStruct, Error> {
        self.begin(27)?;
        self.text(name)?;
        self.length(length)?;
        Ok(Compound { hasher: self, end: 155 })
    }
    fn serialize_struct_variant(self, name: &'static str, index: u32, variant: &'static str, length: usize) -> Result<Self::SerializeStructVariant, Error> {
        self.begin(28)?;
        self.text(name)?;
        self.bytes(&index.to_le_bytes())?;
        self.text(variant)?;
        self.length(length)?;
        Ok(Compound { hasher: self, end: 156 })
    }
    fn collect_str<T: ?Sized + std::fmt::Display>(self, _value: &T) -> Result<(), Error> {
        self.bytes(&[255])?;
        Err(Error::Declined)
    }
}
macro_rules! elements {
    ($trait:ident, $method:ident) => {
        impl ser::$trait for Compound<'_, '_> {
            type Ok = ();
            type Error = Error;
            fn $method<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Error> {
                self.hasher.bytes(&[29])?;
                value.serialize(&mut *self.hasher)
            }
            fn end(self) -> Result<(), Error> { self.hasher.end(self.end) }
        }
    };
}
elements!(SerializeSeq, serialize_element);
elements!(SerializeTuple, serialize_element);
elements!(SerializeTupleStruct, serialize_field);
elements!(SerializeTupleVariant, serialize_field);
macro_rules! fields {
    ($trait:ident) => {
        impl ser::$trait for Compound<'_, '_> {
            type Ok = ();
            type Error = Error;
            fn serialize_field<T: ?Sized + Serialize>(&mut self, key: &'static str, value: &T) -> Result<(), Error> {
                self.hasher.bytes(&[30])?;
                self.hasher.text(key)?;
                value.serialize(&mut *self.hasher)
            }
            fn end(self) -> Result<(), Error> { self.hasher.end(self.end) }
        }
    };
}
fields!(SerializeStruct);
fields!(SerializeStructVariant);

// Bounded source wrapper traversal, serializer frames and the streaming state
// coexist with the proof. This narrows, never raises, its existing scratch cap.
pub(super) const SCRATCH_BYTES: usize = 64 * 1024;

pub(super) fn capture(
    input: Correspondence<'_>,
    prepared: (&model::PreparedValidatedCurve<'static>, f64),
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<Binding>, GeometryError> {
    check_cancel(cancel)?;
    let range = input.curve.range_tuple();
    let active = prepared.0.range();
    let chart = input.chart.range_tuple();
    let map = AffineMap::new(
        [range.0.max(active[0]), range.1.min(active[1])], [chart.0, chart.1],
    )?;
    let mut hasher = Hasher { hash: Sha256::new(), work, cancel, depth: 0 };
    // Owner addresses are used only while this import's immutable,
    // precharged owner vector is alive. Normalized output drops the seal.
    // Map/frame/range/tube bits remain exact; only operand float zeros use
    // their mathematical canonicalization in the serializer above.
    let exact = (
        "retained-forward-operands",
        prepared.0 as *const _ as usize as u64,
        prepared.1.to_bits(),
        active.map(f64::to_bits),
        map.source.map(f64::to_bits),
        map.target.map(f64::to_bits),
        [map.slope.lo.to_bits(), map.slope.hi.to_bits()],
        map.identity,
        input.margin_uv.map(f64::to_bits),
        ("native-curve", input.curve),
        ("native-surface", input.surface),
        ("native-chart", input.chart),
    );
    match exact.serialize(&mut hasher) {
        Ok(()) => Ok(Some(Binding(hasher.hash.finalize().into()))),
        Err(Error::Declined) => Ok(None),
        Err(Error::Geometry(cause)) => Err(cause),
    }
}

fn exact_parameter(value: Interval) -> Option<f64> {
    (value.lo.is_finite() && value.lo == value.hi).then_some(value.lo)
}

pub(super) fn capture_functions(
    input: Correspondence<'_>,
    prepared: (&model::PreparedValidatedCurve<'static>, f64),
    map: AffineMap,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<FunctionBinding>, GeometryError> {
    check_cancel(cancel)?;
    if map.slope.lo != 1.0 || map.slope.hi != 1.0
        || prepared.0.mapped_basis().is_some()
        || !map.source.iter().chain(&map.target).all(|value| value.is_finite())
        || map.source[0] >= map.source[1] || map.target[0] >= map.target[1]
    {
        return Ok(None);
    }
    // Equal slope alone is insufficient: bind BOTH actual endpoint anchors.
    // Point intervals qualify coefficient arithmetic only when it is exact.
    let Some(mut offset) = exact_parameter(
        Interval::point(map.target[0]).sub(Interval::point(map.source[0])),
    ) else { return Ok(None) };
    if exact_parameter(
        Interval::point(map.target[1]).sub(Interval::point(map.source[1])),
    ) != Some(offset) {
        return Ok(None);
    }
    let mut curve = input.curve;
    for depth in 0..=MAX_DEPTH {
        work.charge(1, cancel)?;
        let model::Curve::TrimmedCurve(wrapper) = curve else { break };
        let range = wrapper.entity().range();
        if depth == MAX_DEPTH || !wrapper.orientation()
            || *wrapper.transform() != Matrix4::identity()
            || !range.0.is_finite() || !range.1.is_finite()
            || !(range.0 <= map.source[0] && map.source[1] <= range.1)
        {
            return Ok(None);
        }
        curve = wrapper.entity().curve().as_ref();
    }
    let mut chart = input.chart;
    let mut direction = 1.0;
    let mut parameters = map.target;
    for depth in 0..=MAX_DEPTH {
        work.charge(1, cancel)?;
        let model::Curve2D::TrimmedCurve(wrapper) = chart else { break };
        let range = wrapper.entity().range();
        let low = parameters[0].min(parameters[1]);
        let high = parameters[0].max(parameters[1]);
        if depth == MAX_DEPTH || *wrapper.transform() != Matrix3::identity()
            || !range.0.is_finite() || !range.1.is_finite()
            || !(range.0 <= low && high <= range.1)
        {
            return Ok(None);
        }
        if !wrapper.orientation() {
            let Some(sum) = exact_parameter(
                Interval::point(range.0).add(Interval::point(range.1)),
            ) else { return Ok(None) };
            let Some(next_offset) = exact_parameter(
                Interval::point(sum).sub(Interval::point(offset)),
            ) else { return Ok(None) };
            let Some(first) = exact_parameter(
                Interval::point(sum).sub(Interval::point(parameters[0])),
            ) else { return Ok(None) };
            let Some(last) = exact_parameter(
                Interval::point(sum).sub(Interval::point(parameters[1])),
            ) else { return Ok(None) };
            direction = -direction;
            offset = next_offset;
            parameters = [first, last];
        }
        chart = wrapper.entity().curve().as_ref();
    }
    // This is exact operator identity on the entire contained interval, not
    // agreement at samples or a periodic alias. Reflections also preserve the
    // actual oriented endpoint image, not merely an unordered range.
    if direction != 1.0 || offset != 0.0 || parameters != map.source {
        return Ok(None);
    }
    let mut hasher = Hasher { hash: Sha256::new(), work, cancel, depth: 0 };
    let functions = (
        "retained-forward-restriction-functions",
        prepared.0 as *const _ as usize as u64,
        prepared.1.to_bits(),
        prepared.0.range().map(f64::to_bits),
        ("native-curve", curve),
        ("native-surface", input.surface),
        ("native-chart", chart),
    );
    match functions.serialize(&mut hasher) {
        Ok(()) => Ok(Some(FunctionBinding(hasher.hash.finalize().into()))),
        Err(Error::Declined) => Ok(None),
        Err(Error::Geometry(cause)) => Err(cause),
    }
}
