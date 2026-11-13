// C++ builds its own vtable over inline storage so that one `Convertible`
// can wrap rapidjson values, NSDictionary, QVariant, JNI values, ... The Rust
// equivalent is a trait object: `ConversionTraits` is the C++
// `ConversionTraits<T>` specialization (one impl per front end), and
// `Convertible` holds either a borrowed or an owned `dyn ConversionTraits`.
// Front ends whose members are views into the parent (serde_json) return
// borrowed members; front ends that create new handles per member (JNI) can
// return owned ones.

use feature::Value;

#[derive(Clone, Debug, Default, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct Error {
    pub message: String,
}

impl Error {
    pub fn new(message: impl Into<String>) -> Self {
        Error { message: message.into() }
    }
}

/// The per-front-end operations (`ConversionTraits<T>` in C++). The
/// preconditions are the C++ ones: `array_length`/`array_member` only if
/// `is_array`, `object_member`/`each_member` only if `is_object`.
pub trait ConversionTraits {
    /// Whether the value is undefined or a JSON null.
    fn is_undefined(&self) -> bool;

    fn is_array(&self) -> bool;
    fn array_length(&self) -> usize;
    fn array_member(&self, i: usize) -> Convertible<'_>;

    fn is_object(&self) -> bool;
    fn object_member(&self, name: &str) -> Option<Convertible<'_>>;
    /// Calls `f` once per key and value; stops at, and returns, the first
    /// error `f` returns.
    fn each_member(&self, f: &mut dyn FnMut(&str, &Convertible<'_>) -> Option<Error>) -> Option<Error>;

    fn to_bool(&self) -> Option<bool>;
    /// A JSON number as `float`.
    fn to_number(&self) -> Option<f32>;
    fn to_double(&self) -> Option<f64>;
    fn to_string(&self) -> Option<String>;
    /// A boolean, number or string as a `Value`; numbers prefer `u64`, then
    /// `i64`, then `f64`.
    fn to_value(&self) -> Option<Value>;

    /// Converts the value to GeoJSON. The JSON front end runs
    /// `geojson::convert` and reports its error message in `error`. A front
    /// end without a GeoJSON conversion reports an error (added by `sources`
    /// for the GeoJSON source).
    fn to_geojson(&self, error: &mut Error) -> Option<geojson::GeoJSON> {
        error.message = "GeoJSON conversion is not supported by this value type".to_owned();
        None
    }
}

enum Inner<'a> {
    Borrowed(&'a dyn ConversionTraits),
    Owned(Box<dyn ConversionTraits + 'a>),
}

/// A type-erased, read-only view of a JSON-like value.
pub struct Convertible<'a> {
    inner: Inner<'a>,
}

impl<'a> Convertible<'a> {
    /// Wraps a borrowed front-end value.
    pub fn new(value: &'a dyn ConversionTraits) -> Self {
        Convertible { inner: Inner::Borrowed(value) }
    }

    /// Wraps an owned front-end value (a handle the front end created).
    pub fn owned(value: impl ConversionTraits + 'a) -> Self {
        Convertible { inner: Inner::Owned(Box::new(value)) }
    }

    fn traits(&self) -> &dyn ConversionTraits {
        match &self.inner {
            Inner::Borrowed(v) => *v,
            Inner::Owned(v) => v.as_ref(),
        }
    }

    #[must_use]
    pub fn is_undefined(&self) -> bool {
        self.traits().is_undefined()
    }

    #[must_use]
    pub fn is_array(&self) -> bool {
        self.traits().is_array()
    }

    #[must_use]
    pub fn array_length(&self) -> usize {
        self.traits().array_length()
    }

    #[must_use]
    pub fn array_member(&self, i: usize) -> Convertible<'_> {
        self.traits().array_member(i)
    }

    #[must_use]
    pub fn is_object(&self) -> bool {
        self.traits().is_object()
    }

    #[must_use]
    pub fn object_member(&self, name: &str) -> Option<Convertible<'_>> {
        self.traits().object_member(name)
    }

    pub fn each_member(&self, mut f: impl FnMut(&str, &Convertible<'_>) -> Option<Error>) -> Option<Error> {
        self.traits().each_member(&mut f)
    }

    #[must_use]
    pub fn to_bool(&self) -> Option<bool> {
        self.traits().to_bool()
    }

    #[must_use]
    pub fn to_number(&self) -> Option<f32> {
        self.traits().to_number()
    }

    #[must_use]
    pub fn to_double(&self) -> Option<f64> {
        self.traits().to_double()
    }

    #[must_use]
    pub fn to_string(&self) -> Option<String> {
        self.traits().to_string()
    }

    #[must_use]
    pub fn to_value(&self) -> Option<Value> {
        self.traits().to_value()
    }

    pub fn to_geojson(&self, error: &mut Error) -> Option<geojson::GeoJSON> {
        self.traits().to_geojson(error)
    }
}

impl std::fmt::Debug for Convertible<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Convertible")
    }
}

impl<'a, T: ConversionTraits> From<&'a T> for Convertible<'a> {
    fn from(value: &'a T) -> Self {
        Convertible::new(value)
    }
}

pub fn to_geojson(v: &Convertible<'_>, error: &mut Error) -> Option<geojson::GeoJSON> {
    v.to_geojson(error)
}
