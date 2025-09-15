use serde::Deserialize;

pub trait Overridable {
    type Override: for<'a> Deserialize<'a>;

    fn override_into(self, new: Self::Override) -> Self
    where
        Self: Sized;
}

impl Overridable for () {
    type Override = ();

    fn override_into(self, _: Self::Override) -> Self {
        self
    }
}
