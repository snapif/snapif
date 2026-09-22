#[macro_export]
macro_rules! choice {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $($variant:ident = $wire:literal => $criteria:literal),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        $vis enum $name {
            $($variant),*
        }

        impl $crate::question::ChoiceLabels for $name {
            fn labels() -> &'static [(&'static str, &'static str)] {
                &[$(($wire, $criteria)),*]
            }

            fn from_label(s: &str) -> Option<Self> {
                match s {
                    $($wire => Some(Self::$variant),)*
                    _ => None,
                }
            }

            fn as_label(&self) -> &'static str {
                match self {
                    $(Self::$variant => $wire,)*
                }
            }
        }
    };
}

#[macro_export]
macro_rules! score {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $($variant:ident = $criteria:literal),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        $vis enum $name {
            $($variant),*
        }

        impl $crate::question::ScoreLabels for $name {
            fn criteria() -> &'static [&'static str] {
                &[$($criteria),*]
            }

            fn from_index(i: usize) -> Option<Self> {
                let mut n = 0usize;
                $(
                    if n == i {
                        return Some(Self::$variant);
                    }
                    n += 1;
                )*
                let _ = n;
                None
            }

            fn as_index(&self) -> usize {
                let mut i = 0usize;
                $(
                    if matches!(self, Self::$variant) {
                        return i;
                    }
                    i += 1;
                )*
                i
            }
        }
    };
}
