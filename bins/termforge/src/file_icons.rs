//! Filename-based language marks. Identification is independent of editor/LSP support.
use crate::paint::{hsla, with_alpha};
use gpui::{div, prelude::*, px, AnyElement, FontWeight};
use std::path::Path;
use tf_ui::{Rgb, Theme};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Rust,
    TypeScript,
    JavaScript,
    React,
    Python,
    Go,
    C,
    Cpp,
    CSharp,
    Java,
    Kotlin,
    Swift,
    Ruby,
    Php,
    Html,
    Css,
    Json,
    Yaml,
    Toml,
    Markdown,
    Shell,
    Sql,
    Git,
    Docker,
    Image,
    File,
}

impl FileKind {
    pub fn from_path(path: &Path) -> Self {
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase();
        if name == "dockerfile" || name.starts_with("dockerfile.") || name == "containerfile" {
            return Self::Docker;
        }
        if matches!(
            name.as_str(),
            ".gitignore" | ".gitattributes" | ".gitmodules" | ".gitconfig"
        ) {
            return Self::Git;
        }
        if name == ".env" || name.starts_with(".env.") {
            return Self::Toml;
        }
        if matches!(
            name.as_str(),
            ".bashrc" | ".bash_profile" | ".zshrc" | ".zprofile" | ".profile"
        ) {
            return Self::Shell;
        }
        match path
            .extension()
            .unwrap_or_default()
            .to_string_lossy()
            .to_ascii_lowercase()
            .as_str()
        {
            "rs" => Self::Rust,
            "ts" | "mts" | "cts" => Self::TypeScript,
            "js" | "mjs" | "cjs" => Self::JavaScript,
            "jsx" | "tsx" => Self::React,
            "py" | "pyi" | "pyw" | "ipynb" => Self::Python,
            "go" => Self::Go,
            "c" | "h" => Self::C,
            "cc" | "cpp" | "cxx" | "hpp" | "hxx" | "hh" => Self::Cpp,
            "cs" => Self::CSharp,
            "java" => Self::Java,
            "kt" | "kts" => Self::Kotlin,
            "swift" => Self::Swift,
            "rb" | "rake" | "gemspec" => Self::Ruby,
            "php" => Self::Php,
            "html" | "htm" | "vue" | "svelte" => Self::Html,
            "css" | "scss" | "sass" | "less" => Self::Css,
            "json" | "jsonc" | "json5" => Self::Json,
            "yaml" | "yml" => Self::Yaml,
            "toml" | "ini" | "cfg" | "conf" => Self::Toml,
            "md" | "mdx" | "markdown" | "rst" | "txt" => Self::Markdown,
            "sh" | "bash" | "zsh" | "fish" | "ps1" | "psm1" | "bat" | "cmd" => Self::Shell,
            "sql" => Self::Sql,
            "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "ico" | "avif" => Self::Image,
            _ => Self::File,
        }
    }

    fn mark(self) -> &'static str {
        match self {
            Self::Rust => "Rs",
            Self::TypeScript => "TS",
            Self::JavaScript => "JS",
            Self::React => "Rx",
            Self::Python => "Py",
            Self::Go => "Go",
            Self::C => "C",
            Self::Cpp => "C++",
            Self::CSharp => "C#",
            Self::Java => "Jv",
            Self::Kotlin => "Kt",
            Self::Swift => "Sw",
            Self::Ruby => "Rb",
            Self::Php => "Ph",
            Self::Html => "<>",
            Self::Css => "#",
            Self::Json => "{}",
            Self::Yaml => "Y",
            Self::Toml => "=",
            Self::Markdown => "M↓",
            Self::Shell => ">_",
            Self::Sql => "DB",
            Self::Git => "git",
            Self::Docker => "Dk",
            Self::Image => "Im",
            Self::File => "",
        }
    }

    fn color(self, theme: &Theme) -> Rgb {
        match self {
            Self::Rust | Self::JavaScript | Self::Ruby | Self::Swift => theme.file_script,
            Self::TypeScript
            | Self::React
            | Self::Python
            | Self::Go
            | Self::C
            | Self::Cpp
            | Self::CSharp
            | Self::Java
            | Self::Kotlin
            | Self::Php
            | Self::Docker => theme.file_code,
            Self::Html | Self::Css => theme.file_markup,
            Self::Json | Self::Yaml | Self::Toml | Self::Shell | Self::Sql => theme.file_data,
            Self::Markdown | Self::Git | Self::Image | Self::File => theme.file_document,
        }
    }
}

pub fn file_icon(path: &Path, theme: &Theme) -> AnyElement {
    let kind = FileKind::from_path(path);
    if kind == FileKind::File {
        return crate::icons::icon("□", hsla(theme.file_document));
    }
    let color = hsla(kind.color(theme));
    if kind == FileKind::React {
        return crate::icons::icon("react", color);
    }
    div()
        .size(px(16.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(2.0))
        .border_1()
        .border_color(with_alpha(color, 0.45))
        .text_color(color)
        .font_family(".SystemUIFont")
        .font_weight(FontWeight::SEMIBOLD)
        .text_size(px(if matches!(kind, FileKind::Git | FileKind::Cpp) {
            7.0
        } else {
            8.0
        }))
        .line_height(px(14.0))
        .child(kind.mark())
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identifies_languages_special_filenames_and_unknown_files() {
        for (path, expected) in [
            ("src/main.RS", FileKind::Rust),
            ("src/types.d.ts", FileKind::TypeScript),
            ("src/app.tsx", FileKind::React),
            ("app.jsx", FileKind::React),
            ("index.mjs", FileKind::JavaScript),
            ("types.pyi", FileKind::Python),
            ("main.go", FileKind::Go),
            ("core.hpp", FileKind::Cpp),
            ("App.cs", FileKind::CSharp),
            ("workflow.yml", FileKind::Yaml),
            ("Cargo.toml", FileKind::Toml),
            ("Dockerfile.dev", FileKind::Docker),
            (".gitignore", FileKind::Git),
            (".env.local", FileKind::Toml),
            (".zshrc", FileKind::Shell),
            ("README.md", FileKind::Markdown),
            ("script.ps1", FileKind::Shell),
            ("icon.svg", FileKind::Image),
            ("LICENSE", FileKind::File),
            ("file.unknown", FileKind::File),
        ] {
            assert_eq!(FileKind::from_path(Path::new(path)), expected, "{path}");
        }
    }
}
