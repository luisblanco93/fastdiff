use super::material_icons::ICONS;

pub(super) struct Icon {
    pub name: &'static str,
    pub bytes: &'static [u8],
    pub filenames: &'static [&'static str],
    pub extensions: &'static [&'static str],
}

const FOLDER: Icon = Icon {
    name: "folder-base",
    bytes: include_bytes!("../../assets/material/folder-base.svg"),
    filenames: &[],
    extensions: &[],
};

fn file_icon(path: &[u8]) -> Option<&'static Icon> {
    let filename = path.rsplit(|byte| *byte == b'/').next()?;
    if let Some(icon) = ICONS.iter().find(|icon| {
        icon.filenames
            .iter()
            .any(|name| filename.eq_ignore_ascii_case(name.as_bytes()))
    }) {
        return Some(icon);
    }
    ICONS
        .iter()
        .flat_map(|icon| {
            icon.extensions
                .iter()
                .map(move |extension| (icon, extension))
        })
        .filter(|(_, extension)| {
            filename.len() > extension.len()
                && filename[filename.len() - extension.len() - 1] == b'.'
                && filename[filename.len() - extension.len()..]
                    .eq_ignore_ascii_case(extension.as_bytes())
        })
        .max_by_key(|(_, extension)| extension.len())
        .map(|(icon, _)| icon)
}

pub(super) fn paint(ui: &egui::Ui, rect: egui::Rect, path: Option<&[u8]>) -> bool {
    let icon = match path {
        Some(path) => file_icon(path),
        None => Some(&FOLDER),
    };
    if let Some(icon) = icon {
        egui::Image::from_bytes(format!("bytes://material/{}.svg", icon.name), icon.bytes)
            .paint_at(ui, rect);
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages_templates_and_data_use_material_icons() {
        for (path, expected) in [
            ("src/main.rs", "rust"),
            ("main.go", "go"),
            ("a.c", "c"),
            ("a.cpp", "cpp"),
            ("a.py", "python"),
            ("a.js", "javascript"),
            ("a.ts", "typescript"),
            ("a.tsx", "react_ts"),
            ("a.css", "css"),
            ("a.sh", "console"),
            ("a.ps1", "powershell"),
            ("a.j2", "jinja"),
            ("a.html", "html"),
            ("README.md", "markdown"),
            ("a.json", "json"),
            ("a.yaml", "yaml"),
            ("a.toml", "toml"),
            ("a.sql", "database"),
            ("a.d.ts", "typescript-def"),
            ("a.schema.json", "json_schema"),
            ("SCRIPT.PY", "python"),
            (".jshintrc", "json"),
            (".htaccess", "xml"),
            (".pug-lintrc.js", "pug"),
        ] {
            assert_eq!(
                file_icon(path.as_bytes()).map(|icon| icon.name),
                Some(expected),
                "{path}"
            );
        }
        for path in [b"unknown.zzz".as_slice(), b"image.png", b"archive.zip"] {
            assert!(file_icon(path).is_none());
        }
    }

    #[test]
    fn raw_non_unicode_filenames_keep_their_extension_and_identity() {
        assert_eq!(file_icon(b"dir/\xff.py").unwrap().name, "python");
        assert!(file_icon(b"dir/\xff").is_none());
        assert_eq!(
            file_icon(b"dir/\xff.schema.json").unwrap().name,
            "json_schema"
        );
    }

    #[test]
    fn every_bundled_svg_decodes_at_tree_size() {
        for icon in super::super::material_icons::ICONS
            .iter()
            .chain(std::iter::once(&FOLDER))
        {
            let image = egui_extras::image::load_svg_bytes_with_size(
                icon.bytes,
                egui::SizeHint::Height(14),
                &Default::default(),
            )
            .unwrap_or_else(|error| panic!("{}: {error}", icon.name));
            assert_eq!(image.size[1], 14, "{}", icon.name);
            assert!(image.size[0] > 0, "{}", icon.name);
            assert!(
                image.pixels.iter().any(|pixel| pixel.a() > 0),
                "{}",
                icon.name
            );
        }
    }
}
