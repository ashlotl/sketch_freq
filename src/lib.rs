pub mod frontend;
pub mod lv2_impl;
pub mod shared_data;

use lv2::prelude::*;

use crate::lv2_impl::Amp;

lv2_descriptors!(Amp);
