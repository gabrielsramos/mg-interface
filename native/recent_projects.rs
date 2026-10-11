use std::{
    fs,
    path::{Path, PathBuf},
};

pub const MAX_RECENT_PROJECTS: usize = 10;

pub struct RecentProjects {
    file: PathBuf,
    paths: Vec<PathBuf>,
}

impl RecentProjects {
    pub fn load(directory: &Path) -> (Self, Option<String>) {
        let file = directory.join("recent-projects.json");
        let result = match fs::read(&file) {
            Ok(bytes) => serde_json::from_slice::<Vec<PathBuf>>(&bytes).map_err(|e| e.to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e.to_string()),
        };
        let (paths, warning) = match result {
            Ok(paths) => {
                let mut unique = Vec::new();
                for path in paths {
                    if !unique.contains(&path) {
                        unique.push(path);
                    }
                    if unique.len() == MAX_RECENT_PROJECTS {
                        break;
                    }
                }
                (unique, None)
            }
            Err(e) => (
                Vec::new(),
                Some(format!("Could not read recent projects: {e}")),
            ),
        };
        (Self { file, paths }, warning)
    }

    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    pub fn remember(&mut self, path: PathBuf) -> Result<(), String> {
        self.paths.retain(|existing| existing != &path);
        self.paths.insert(0, path);
        self.paths.truncate(MAX_RECENT_PROJECTS);
        let bytes = serde_json::to_vec_pretty(&self.paths).map_err(|e| e.to_string())?;
        fs::create_dir_all(self.file.parent().ok_or("No state directory")?)
            .map_err(|e| e.to_string())?;
        fs::write(&self.file, bytes)
            .map_err(|e| format!("Project loaded, but recent projects could not be saved: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_directory(label: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("recent-project-tests")
            .join(format!("{label}-{}-{unique}", std::process::id()))
    }

    #[test]
    fn history_survives_restart_and_reopening_moves_project_to_front() {
        let directory = test_directory("history");
        let (mut history, warning) = RecentProjects::load(&directory);
        assert!(warning.is_none());
        assert!(history.paths().is_empty());
        let projects: Vec<_> = (0..12)
            .map(|i| directory.join(format!("Game with spaces á {i}.csproj")))
            .collect();
        for project in &projects {
            history.remember(project.clone()).unwrap();
        }
        history.remember(projects[5].clone()).unwrap();
        let (restored, warning) = RecentProjects::load(&directory);
        assert!(warning.is_none());
        assert_eq!(restored.paths().len(), MAX_RECENT_PROJECTS);
        assert_eq!(restored.paths().first(), Some(&projects[5]));
        assert_eq!(
            restored
                .paths()
                .iter()
                .filter(|path| **path == projects[5])
                .count(),
            1
        );
        assert!(!restored.paths().contains(&projects[0]));
    }

    #[test]
    fn damaged_history_does_not_prevent_loading_new_projects() {
        let directory = test_directory("damaged");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("recent-projects.json"), b"invalid JSON").unwrap();
        let (mut history, warning) = RecentProjects::load(&directory);
        assert!(warning.is_some());
        assert!(history.paths().is_empty());
        let project = directory.join("Recovered.csproj");
        history.remember(project.clone()).unwrap();
        let (restored, warning) = RecentProjects::load(&directory);
        assert!(warning.is_none());
        assert_eq!(restored.paths(), &[project]);
    }
}
