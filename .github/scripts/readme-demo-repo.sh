#!/usr/bin/env bash
# Builds the small demo repository shown in the README screenshots: pushed history, two
# unpushed commits, a file with both staged and unstaged edits, a staged new file, an
# untracked file, and a branch checkout for the reflog.
# usage: readme-demo-repo.sh <parent-dir>   (creates <parent-dir>/tasks and <parent-dir>/origin.git)
set -euo pipefail
parent="$1"
rm -rf "$parent"
mkdir -p "$parent"
git init -q --bare -b main "$parent/origin.git"
repo="$parent/tasks"
mkdir -p "$repo/src" "$repo/tests" "$repo/docs"
cd "$repo"
git init -q -b main
git config user.name "Demo"
git config user.email "demo@example.invalid"
git remote add origin "$parent/origin.git"

commit() { git add -A && git commit -qm "$1"; }

cat > Cargo.toml <<'EOF'
[package]
name = "tasks"
version = "0.3.0"
edition = "2021"
EOF

cat > src/lib.rs <<'EOF'
pub mod api;
pub mod store;

pub use store::{Task, TaskStore};
EOF

cat > README.md <<'EOF'
# tasks

A tiny task tracker used to demo **fastdiff**.

## Usage

```rust
let mut store = TaskStore::default();
let id = store.add("Write the release notes");
store.complete(id);
```
EOF
commit "Initial project layout"

cat > src/store.rs <<'EOF'
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub struct Task {
    pub id: u32,
    pub title: String,
    pub done: bool,
}

#[derive(Default)]
pub struct TaskStore {
    next_id: u32,
    tasks: BTreeMap<u32, Task>,
}

impl TaskStore {
    pub fn add(&mut self, title: &str) -> u32 {
        self.next_id += 1;
        let task = Task {
            id: self.next_id,
            title: title.to_owned(),
            done: false,
        };
        self.tasks.insert(task.id, task);
        self.next_id
    }

    pub fn complete(&mut self, id: u32) -> bool {
        match self.tasks.get_mut(&id) {
            Some(task) => {
                task.done = true;
                true
            }
            None => false,
        }
    }

    pub fn open(&self) -> Vec<&Task> {
        let tasks = self.tasks.values();
        tasks.filter(|t| !t.done).collect()
    }
}
EOF
commit "Add the in-memory task store"

cat > src/api.rs <<'EOF'
use crate::store::TaskStore;

pub enum Request {
    Add(String),
    Complete(u32),
    List,
}

pub fn handle(store: &mut TaskStore, request: Request) -> String {
    match request {
        Request::Add(title) => {
            let id = store.add(&title);
            format!("added #{id}")
        }
        Request::Complete(id) => {
            if store.complete(id) {
                format!("completed #{id}")
            } else {
                format!("no task #{id}")
            }
        }
        Request::List => store
            .open()
            .iter()
            .map(|t| format!("#{} {}", t.id, t.title))
            .collect::<Vec<_>>()
            .join("\n"),
    }
}
EOF
commit "Add request handling"
git push -q -u origin main

# Unpushed work, made on a branch and fast-forwarded so the reflog shows a checkout and a merge.
git checkout -q -b due-dates
python3 - <<'PY'
from pathlib import Path
store = Path("src/store.rs")
text = store.read_text()
text = text.replace("    pub done: bool,\n}", "    pub done: bool,\n    pub due: Option<u32>,\n}")
text = text.replace("pub fn add(&mut self, title: &str) -> u32 {", "pub fn add(&mut self, title: &str, due: Option<u32>) -> u32 {")
text = text.replace("            done: false,\n", "            done: false,\n            due,\n")
text = text.replace("""    pub fn open(&self) -> Vec<&Task> {""", """    /// Open tasks due on or before `day`.
    pub fn due_by(&self, day: u32) -> Vec<&Task> {
        let open = self.open().into_iter();
        open.filter(|t| t.due.is_some_and(|d| d <= day))
            .collect()
    }

    pub fn open(&self) -> Vec<&Task> {""")
store.write_text(text)
api = Path("src/api.rs")
text = api.read_text()
text = text.replace("    Add(String),", "    Add(String, Option<u32>),")
text = text.replace("Request::Add(title) => {\n            let id = store.add(&title);", "Request::Add(title, due) => {\n            let id = store.add(&title, due);")
api.write_text(text)
PY
commit "Add due dates to tasks"
cat >> README.md <<'EOF'

## Due dates

Pass a day number to `add`, then ask for what is due:

| Call | Returns |
| --- | --- |
| `store.add("Ship", Some(12))` | the new task id |
| `store.due_by(12)` | open tasks due by day 12 |

> Days are plain numbers for now; calendar dates come later.
EOF
perl -pi -e 's/\Qstore.add("Write the release notes");\E/store.add("Write the release notes", Some(3));/' README.md
commit "Document due dates"
git checkout -q main
git merge -q --ff-only due-dates

# Staged: a new request in the API and a new test file.
python3 - <<'PY'
from pathlib import Path
api = Path("src/api.rs")
text = api.read_text()
text = text.replace("    Complete(u32),\n", "    Complete(u32),\n    Remove(u32),\n")
text = text.replace("        Request::List => store", """        Request::Remove(id) => match store.remove(id) {
            Some(task) => format!("removed {}", task.title),
            None => format!("no task #{id}"),
        },
        Request::List => store""")
api.write_text(text)
PY
cat > tests/api.rs <<'EOF'
use tasks::api::{handle, Request};
use tasks::TaskStore;

#[test]
fn removes_a_task() {
    let mut store = TaskStore::default();
    handle(&mut store, Request::Add("Ship".into(), None));
    let reply = handle(&mut store, Request::Remove(1));
    assert_eq!(reply, "removed Ship");
}
EOF
git add src/api.rs tests/api.rs

# Unstaged: friendlier replies on top of the staged change, and the store method it needs.
python3 - <<'PY'
from pathlib import Path
api = Path("src/api.rs")
text = api.read_text()
text = text.replace('format!("added #{id}")', 'format!("added task #{id}")')
text = text.replace('format!("no task #{id}")', 'format!("task #{id} not found")')
api.write_text(text)
store = Path("src/store.rs")
text = store.read_text()
text = text.replace("    pub fn open(&self) -> Vec<&Task> {", """    pub fn remove(&mut self, id: u32) -> Option<Task> {
        self.tasks.remove(&id)
    }

    pub fn open(&self) -> Vec<&Task> {""")
store.write_text(text)
PY

# Untracked.
cat > docs/roadmap.md <<'EOF'
# Roadmap

- Calendar dates instead of day numbers
- Persist tasks to disk
EOF

git status --short
git log --oneline --all
