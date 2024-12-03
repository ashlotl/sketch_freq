pub mod aabb;
pub mod frontend;
pub mod lv2_impl;
pub mod shared_data;

use lv2::prelude::*;

use crate::lv2_impl::SynthWrapper;

lv2_descriptors!(SynthWrapper);
