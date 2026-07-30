use std::borrow::Cow;

use tokenx_engine::GroupBy;

use crate::theme::ThemeName;

pub(crate) fn group_by_label(value: GroupBy) -> Cow<'static, str> {
    group_by_label_for_locale(value, &rust_i18n::locale())
}

fn group_by_label_for_locale(value: GroupBy, locale: &str) -> Cow<'static, str> {
    let key = match value {
        GroupBy::Model => "tui.model.display.group_by.model",
        GroupBy::ClientModel => "tui.model.display.group_by.client_model",
        GroupBy::ClientProviderModel => "tui.model.display.group_by.client_provider_model",
        GroupBy::WorkspaceModel => "tui.model.display.group_by.workspace_model",
    };
    rust_i18n::t!(key, locale = locale)
}

pub(crate) fn theme_label(value: ThemeName) -> Cow<'static, str> {
    theme_label_for_locale(value, &rust_i18n::locale())
}

fn theme_label_for_locale(value: ThemeName, locale: &str) -> Cow<'static, str> {
    let key = match value {
        ThemeName::Green => "tui.model.display.theme.green",
        ThemeName::Halloween => "tui.model.display.theme.halloween",
        ThemeName::Teal => "tui.model.display.theme.teal",
        ThemeName::Blue => "tui.model.display.theme.blue",
        ThemeName::Pink => "tui.model.display.theme.pink",
        ThemeName::Purple => "tui.model.display.theme.purple",
        ThemeName::Orange => "tui.model.display.theme.orange",
        ThemeName::Monochrome => "tui.model.display.theme.monochrome",
        ThemeName::YlGnBu => "tui.model.display.theme.ylgnbu",
        ThemeName::Graphite => "tui.model.display.theme.graphite",
        ThemeName::Lagoon => "tui.model.display.theme.lagoon",
        ThemeName::Dusk => "tui.model.display.theme.dusk",
    };
    rust_i18n::t!(key, locale = locale)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chinese_group_by_labels_are_exhaustive_and_user_facing() {
        let cases = [
            (GroupBy::Model, "模型"),
            (GroupBy::ClientModel, "客户端+模型"),
            (GroupBy::ClientProviderModel, "客户端+提供商+模型"),
            (GroupBy::WorkspaceModel, "工作区+模型"),
        ];

        for (value, expected) in cases {
            assert_eq!(group_by_label_for_locale(value, "zh-CN"), expected);
        }
    }

    #[test]
    fn english_group_by_labels_preserve_canonical_cli_spelling() {
        for value in [
            GroupBy::Model,
            GroupBy::ClientModel,
            GroupBy::ClientProviderModel,
            GroupBy::WorkspaceModel,
        ] {
            assert_eq!(group_by_label_for_locale(value, "en"), value.to_string());
        }
    }

    #[test]
    fn chinese_theme_labels_are_exhaustive_and_user_facing() {
        let cases = [
            (ThemeName::Green, "绿色"),
            (ThemeName::Halloween, "万圣节"),
            (ThemeName::Teal, "青色"),
            (ThemeName::Blue, "蓝色"),
            (ThemeName::Pink, "粉色"),
            (ThemeName::Purple, "紫色"),
            (ThemeName::Orange, "橙色"),
            (ThemeName::Monochrome, "单色"),
            (ThemeName::YlGnBu, "黄绿蓝"),
            (ThemeName::Graphite, "石墨"),
            (ThemeName::Lagoon, "泻湖"),
            (ThemeName::Dusk, "暮色"),
        ];

        for (value, expected) in cases {
            assert_eq!(theme_label_for_locale(value, "zh-CN"), expected);
        }
    }

    #[test]
    fn english_theme_labels_preserve_canonical_settings_spelling() {
        for &value in ThemeName::all() {
            assert_eq!(theme_label_for_locale(value, "en"), value.as_str());
        }
    }
}
