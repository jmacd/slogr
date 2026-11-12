// Copyright The OpenTelemetry Authors
// SPDX-License-Identifier: Apache-2.0

//! Static, typed attribute declarations.

use std::{borrow::Borrow, marker::PhantomData};

/// A named attribute whose values must match `T` at every logging callsite.
///
/// The descriptor allocates nothing. Its name becomes the encoded key; its
/// description is retained but is not emitted. Declare descriptors as statics
/// and import or alias them using ordinary Rust `use` declarations.
///
/// ```
/// use slogr::{Attribute, otel_info};
///
/// mod attributes {
///     use slogr::Attribute;
///     pub static COUNT: Attribute<u64> = Attribute::new("item.count", "Number of items");
/// }
/// use attributes::COUNT as my_attribute;
///
/// otel_info!("Items received", my_attribute = 42u64);
/// ```
///
/// `Attribute<str>` accepts any [`AsRef<str>`] input, including `String`,
/// `&String`, `Cow<str>`, and custom string wrappers, without allocating.
/// Other attributes require [`Borrow<T>`], not a numeric conversion.
/// The borrowed `T` is what is encoded through tracing's native `Value` interface.
///
/// A mismatched type fails even if the event would be filtered out:
///
/// ```compile_fail
/// use slogr::{Attribute, otel_debug};
/// static COUNT: Attribute<u64> = Attribute::new("count", "Number of items");
/// otel_debug!("Items received", COUNT = 42u32);
/// ```
///
/// Free-form keys are not attribute declarations:
///
/// ```compile_fail
/// slogr::otel_info!("Items received", undeclared_count = 42u64);
/// ```
///
/// Formatting overrides are not supported:
///
/// ```compile_fail
/// use slogr::{Attribute, otel_info};
/// static COUNT: Attribute<u64> = Attribute::new("count", "Number of items");
/// otel_info!("Items received", COUNT = %42u64);
/// ```
///
/// `message` has no special meaning; it must resolve to a descriptor too:
///
/// ```compile_fail
/// slogr::otel_info!("Items received", message = "another body");
/// ```
pub struct Attribute<T: ?Sized> {
    name: &'static str,
    description: &'static str,
    // Describes a borrowed type without owning T or requiring T: Send + Sync.
    value_type: PhantomData<fn(&T)>,
}

impl<T: ?Sized> Attribute<T> {
    /// Declare an attribute with a wire name and an unused description.
    #[must_use]
    pub const fn new(name: &'static str, description: &'static str) -> Self {
        Self {
            name,
            description,
            value_type: PhantomData,
        }
    }

    /// The key written to the encoded log record.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        self.name
    }

    /// Descriptive metadata, not currently emitted.
    #[must_use]
    pub const fn description(&self) -> &'static str {
        self.description
    }
}

impl Attribute<str> {
    #[doc(hidden)]
    #[inline]
    pub fn __value<'a, V: AsRef<str> + ?Sized>(&self, value: &'a V) -> &'a str {
        value.as_ref()
    }
}

/// Fallback binding for non-string attributes; strings use the inherent AsRef method.
#[doc(hidden)]
pub trait AttributeValue<T: ?Sized> {
    fn __value<'a, V: Borrow<T> + ?Sized>(&self, value: &'a V) -> &'a T;
}

impl<T: ?Sized> AttributeValue<T> for Attribute<T> {
    #[inline]
    fn __value<'a, V: Borrow<T> + ?Sized>(&self, value: &'a V) -> &'a T {
        value.borrow()
    }
}
