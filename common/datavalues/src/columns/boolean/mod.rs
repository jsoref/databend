// Copyright 2021 Datafuse Labs.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::sync::Arc;

use common_arrow::arrow::array::*;
use common_arrow::arrow::bitmap::utils::BitChunkIterExact;
use common_arrow::arrow::bitmap::utils::BitChunksExact;
use common_arrow::arrow::bitmap::Bitmap;
use common_arrow::arrow::datatypes::DataType as ArrowType;
use common_arrow::bitmap::MutableBitmap;

use crate::prelude::*;

mod iterator;
mod mutable;

pub use iterator::*;
pub use mutable::*;

#[derive(Clone)]
pub struct BooleanColumn {
    values: Bitmap,
}

impl From<BooleanArray> for BooleanColumn {
    fn from(array: BooleanArray) -> Self {
        Self::new(array)
    }
}

impl BooleanColumn {
    pub fn new(array: BooleanArray) -> Self {
        Self {
            values: array.values().clone(),
        }
    }

    pub fn from_arrow_array(array: &dyn Array) -> Self {
        Self::new(
            array
                .as_any()
                .downcast_ref::<BooleanArray>()
                .unwrap()
                .clone(),
        )
    }

    pub fn from_arrow_data(values: Bitmap) -> Self {
        Self::from_arrow_array(&BooleanArray::from_data(ArrowType::Boolean, values, None))
    }

    pub fn values(&self) -> &Bitmap {
        &self.values
    }
}

impl Column for BooleanColumn {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn data_type(&self) -> DataTypeImpl {
        BooleanType::new_impl()
    }

    fn column_type_name(&self) -> String {
        "Boolean".to_string()
    }

    fn len(&self) -> usize {
        self.values.len()
    }

    fn memory_size(&self) -> usize {
        self.values.as_slice().0.len()
    }

    fn as_arrow_array(&self) -> ArrayRef {
        let array = BooleanArray::from_data(ArrowType::Boolean, self.values.clone(), None);
        Arc::new(array)
    }

    fn arc(&self) -> ColumnRef {
        Arc::new(self.clone())
    }

    fn slice(&self, offset: usize, length: usize) -> ColumnRef {
        assert!(
            offset + length <= self.len(),
            "the offset of the new Buffer cannot exceed the existing length"
        );
        unsafe {
            Arc::new(Self {
                values: self.values.clone().slice_unchecked(offset, length),
            })
        }
    }

    fn filter(&self, filter: &BooleanColumn) -> ColumnRef {
        let selected = filter.values().len() - filter.values().null_count();
        if selected == self.len() {
            return Arc::new(self.clone());
        }
        let mut bitmap = MutableBitmap::with_capacity(selected);
        let (value_slice, _, value_length) = self.values().as_slice();
        let (slice, _, length) = filter.values().as_slice();

        let mut chunks = BitChunksExact::<u64>::new(value_slice, value_length);
        let mut mask_chunks = BitChunksExact::<u64>::new(slice, length);

        chunks
            .by_ref()
            .zip(mask_chunks.by_ref())
            .for_each(|(chunk, mut mask)| {
                while mask != 0 {
                    let n = mask.trailing_zeros() as usize;
                    let value: bool = chunk & (1 << n) != 0;
                    bitmap.push(value);
                    mask = mask & (mask - 1);
                }
            });

        chunks
            .remainder_iter()
            .zip(mask_chunks.remainder_iter())
            .for_each(|(value, is_selected)| {
                if is_selected {
                    bitmap.push(value);
                }
            });

        let col = BooleanColumn {
            values: bitmap.into(),
        };
        Arc::new(col)
    }

    fn scatter(&self, indices: &[usize], scattered_size: usize) -> Vec<ColumnRef> {
        let mut builders = Vec::with_capacity(scattered_size);
        for _i in 0..scattered_size {
            builders.push(MutableBooleanColumn::with_capacity(self.len()));
        }

        indices
            .iter()
            .zip(self.values())
            .for_each(|(index, value)| {
                builders[*index].append_value(value);
            });

        builders.iter_mut().map(|b| b.to_column()).collect()
    }

    fn replicate(&self, offsets: &[usize]) -> ColumnRef {
        debug_assert!(
            offsets.len() == self.len(),
            "Size of offsets must match size of column"
        );

        if offsets.is_empty() {
            return self.slice(0, 0);
        }

        let mut builder = MutableBooleanColumn::with_capacity(*offsets.last().unwrap());

        let mut previous_offset: usize = 0;

        (0..self.len()).for_each(|i| {
            let offset: usize = offsets[i];
            let data = self.values.get_bit(i);
            builder
                .values
                .extend_constant(offset - previous_offset, data);
            previous_offset = offset;
        });

        builder.to_column()
    }

    fn convert_full_column(&self) -> ColumnRef {
        Arc::new(self.clone())
    }

    fn get(&self, index: usize) -> DataValue {
        DataValue::Boolean(self.values.get_bit(index))
    }
}

impl ScalarColumn for BooleanColumn {
    type Builder = MutableBooleanColumn;
    type OwnedItem = bool;
    type RefItem<'a> = bool;
    type Iterator<'a> = BitmapValuesIter<'a>;

    #[inline]
    fn get_data(&self, idx: usize) -> Self::RefItem<'_> {
        self.values.get_bit(idx)
    }

    fn scalar_iter(&self) -> Self::Iterator<'_> {
        self.iter()
    }

    fn from_slice(data: &[Self::RefItem<'_>]) -> Self {
        let bitmap = MutableBitmap::from_iter(data.as_ref().iter().cloned());
        BooleanColumn {
            values: bitmap.into(),
        }
    }

    fn from_iterator<'a>(it: impl Iterator<Item = Self::RefItem<'a>>) -> Self {
        let bitmap = MutableBitmap::from_iter(it);
        BooleanColumn {
            values: bitmap.into(),
        }
    }

    fn from_owned_iterator(it: impl Iterator<Item = Self::OwnedItem>) -> Self {
        let bitmap = match it.size_hint() {
            (_, Some(_)) => unsafe { MutableBitmap::from_trusted_len_iter_unchecked(it) },
            (_, None) => MutableBitmap::from_iter(it),
        };
        BooleanColumn {
            values: bitmap.into(),
        }
    }
}

impl std::fmt::Debug for BooleanColumn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let iter = self.iter().map(|x| if x { "true" } else { "false" });
        let head = "BooleanColumn";
        display_fmt(iter, head, self.len(), self.data_type_id(), f)
    }
}
