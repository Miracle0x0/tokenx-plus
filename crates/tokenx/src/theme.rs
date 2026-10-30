use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ThemeName {
    Green,
    Halloween,
    Teal,
    Blue,
    Pink,
    Purple,
    Orange,
    Monochrome,
    YlGnBu,
    Graphite,
    Lagoon,
    Dusk,
    InkstoneXuanmo,
    InkstoneXuanmoDimmed,
    InkstoneXuanzhi,
    InkstoneXuanzhiDimmed,
    InkstoneCanghai,
    InkstoneQingci,
}

impl ThemeName {
    pub(crate) fn all() -> &'static [ThemeName] {
        &[
            ThemeName::Green,
            ThemeName::Halloween,
            ThemeName::Teal,
            ThemeName::Blue,
            ThemeName::Pink,
            ThemeName::Purple,
            ThemeName::Orange,
            ThemeName::Monochrome,
            ThemeName::YlGnBu,
            ThemeName::Graphite,
            ThemeName::Lagoon,
            ThemeName::Dusk,
            ThemeName::InkstoneXuanmo,
            ThemeName::InkstoneXuanmoDimmed,
            ThemeName::InkstoneXuanzhi,
            ThemeName::InkstoneXuanzhiDimmed,
            ThemeName::InkstoneCanghai,
            ThemeName::InkstoneQingci,
        ]
    }

    pub(crate) fn next(self) -> ThemeName {
        let themes = Self::all();
        let idx = themes.iter().position(|&theme| theme == self).unwrap_or(0);
        themes[(idx + 1) % themes.len()]
    }

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            ThemeName::Green => "green",
            ThemeName::Halloween => "halloween",
            ThemeName::Teal => "teal",
            ThemeName::Blue => "blue",
            ThemeName::Pink => "pink",
            ThemeName::Purple => "purple",
            ThemeName::Orange => "orange",
            ThemeName::Monochrome => "monochrome",
            ThemeName::YlGnBu => "ylgnbu",
            ThemeName::Graphite => "graphite",
            ThemeName::Lagoon => "lagoon",
            ThemeName::Dusk => "dusk",
            ThemeName::InkstoneXuanmo => "inkstone-xuanmo",
            ThemeName::InkstoneXuanmoDimmed => "inkstone-xuanmo-dimmed",
            ThemeName::InkstoneXuanzhi => "inkstone-xuanzhi",
            ThemeName::InkstoneXuanzhiDimmed => "inkstone-xuanzhi-dimmed",
            ThemeName::InkstoneCanghai => "inkstone-canghai",
            ThemeName::InkstoneQingci => "inkstone-qingci",
        }
    }

    fn from_canonical(value: &str) -> Option<Self> {
        Self::all()
            .iter()
            .copied()
            .find(|theme| theme.as_str() == value)
    }

    fn valid_list() -> String {
        Self::all()
            .iter()
            .map(|theme| theme.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

impl std::str::FromStr for ThemeName {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_canonical(s).ok_or_else(|| {
            rust_i18n::t!(
                "theme.error.invalid_theme",
                name = s,
                valid = Self::valid_list()
            )
            .into_owned()
        })
    }
}

impl Serialize for ThemeName {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ThemeName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_canonical(&value).ok_or_else(|| {
            serde::de::Error::custom(rust_i18n::t!(
                "theme.error.unknown_theme",
                name = value,
                valid = Self::valid_list()
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::ThemeName;

    #[test]
    fn inkstone_names_round_trip_through_settings_and_cycle() {
        let themes = [
            ThemeName::InkstoneXuanmo,
            ThemeName::InkstoneXuanmoDimmed,
            ThemeName::InkstoneXuanzhi,
            ThemeName::InkstoneXuanzhiDimmed,
            ThemeName::InkstoneCanghai,
            ThemeName::InkstoneQingci,
        ];
        for (index, &theme) in themes.iter().enumerate() {
            let json = serde_json::to_string(&theme).unwrap();
            assert_eq!(serde_json::from_str::<ThemeName>(&json).unwrap(), theme);
            assert_eq!(
                theme.next(),
                themes.get(index + 1).copied().unwrap_or(ThemeName::Green)
            );
        }
    }
}
