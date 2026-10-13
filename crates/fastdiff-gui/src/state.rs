#[derive(Debug)]
pub struct Preferences {
    pub unified: bool,
    pub theme: bool,
    pub font_size: f32,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            unified: false,
            theme: false,
            font_size: 13.0,
        }
    }
}

pub fn filtered_paths(paths: &[String], filter: &str) -> Vec<usize> {
    let filter = filter.to_lowercase();
    paths
        .iter()
        .enumerate()
        .filter_map(|(i, path)| path.to_lowercase().contains(&filter).then_some(i))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_filter_is_case_insensitive_and_keeps_stable_indices() {
        let paths = vec![
            "src/main.rs".into(),
            "README.md".into(),
            "src/app.rs".into(),
        ];
        assert_eq!(filtered_paths(&paths, "SRC/"), vec![0, 2]);
        assert_eq!(filtered_paths(&paths, "no match"), Vec::<usize>::new());
    }
}
