use std::{
    collections::{HashMap, HashSet},
    sync::Mutex,
};

use serde::Serialize;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct GroupInfo {
    pub group_id: Uuid,
    pub group_name: String,
    pub state: String,
    pub participants: Vec<String>,
}

#[derive(Debug)]
struct Group {
    id: Uuid,
    name: String,
    members: HashSet<String>,
}

impl Group {
    fn info(&self) -> GroupInfo {
        let mut participants: Vec<_> = self.members.iter().cloned().collect();
        participants.sort();

        GroupInfo {
            group_id: self.id,
            group_name: self.name.clone(),
            state: "Idle".to_owned(),
            participants,
        }
    }
}

#[derive(Default)]
struct Inner {
    groups: HashMap<Uuid, Group>,
    // Remux currently uses device.id as its Jellyfin session ID.
    device_groups: HashMap<String, Uuid>,
}

#[derive(Default)]
pub struct SyncPlayManager {
    inner: Mutex<Inner>,
}

impl SyncPlayManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn create(&self, device_id: &str, name: String) -> GroupInfo {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        leave_locked(&mut inner, device_id);

        let id = Uuid::new_v4();
        let group = Group {
            id,
            name,
            members: HashSet::from([device_id.to_owned()]),
        };
        let info = group.info();

        inner.device_groups.insert(device_id.to_owned(), id);
        inner.groups.insert(id, group);
        info
    }

    pub fn list(&self) -> Vec<GroupInfo> {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let mut groups: Vec<_> = inner.groups.values().map(Group::info).collect();
        groups.sort_by(|a, b| a.group_name.cmp(&b.group_name));
        groups
    }

    pub fn get(&self, id: Uuid) -> Option<GroupInfo> {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.groups.get(&id).map(Group::info)
    }

    pub fn join(&self, device_id: &str, id: Uuid) -> Option<GroupInfo> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if !inner.groups.contains_key(&id) {
            return None;
        }

        leave_locked(&mut inner, device_id);
        let group = inner.groups.get_mut(&id)?;
        group.members.insert(device_id.to_owned());
        let info = group.info();
        inner.device_groups.insert(device_id.to_owned(), id);
        Some(info)
    }

    pub fn leave(&self, device_id: &str) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        leave_locked(&mut inner, device_id);
    }
}

fn leave_locked(inner: &mut Inner, device_id: &str) {
    if let Some(id) = inner.device_groups.remove(device_id) {
        if let Some(group) = inner.groups.get_mut(&id) {
            group.members.remove(device_id);
            if group.members.is_empty() {
                inner.groups.remove(&id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_lifecycle() {
        let manager = SyncPlayManager::new();

        let group = manager.create("host", "Watch party".to_owned());
        assert_eq!(manager.list().len(), 1);
        assert_eq!(group.group_name, "Watch party");
        assert_eq!(group.participants, vec!["host"]);

        let joined = manager.join("friend", group.group_id).unwrap();
        assert_eq!(joined.participants, vec!["friend", "host"]);

        manager.leave("host");
        assert_eq!(
            manager.get(group.group_id).unwrap().participants,
            vec!["friend"]
        );

        manager.leave("friend");
        assert!(manager.list().is_empty());
    }
}