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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupQueue {
    pub item_ids: Vec<Uuid>,
	pub playlist_item_ids: Vec<Uuid>,
    pub playing_index: usize,
    pub position_ticks: i64,
}

#[derive(Debug)]
struct Group {
    id: Uuid,
    name: String,
    members: HashSet<String>,
	queue: Option<GroupQueue>,
	state: String,
	ready_members: HashSet<String>,
	play_started_at: Option<chrono::DateTime<chrono::Utc>>,

}

impl Group {
    fn info(&self) -> GroupInfo {
        let mut participants: Vec<_> = self.members.iter().cloned().collect();
        participants.sort();

        GroupInfo {
            group_id: self.id,
            group_name: self.name.clone(),
            state: self.state.clone(),
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
			queue: None,
			state: "Idle".to_owned(),
			ready_members: HashSet::new(),
			play_started_at: None,
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
		if inner.device_groups.get(device_id) == Some(&id) {
			return inner.groups.get(&id).map(Group::info);
		}

        leave_locked(&mut inner, device_id);

		let group = inner.groups.get_mut(&id)?;
		group.members.insert(device_id.to_owned());
		group.ready_members.remove(device_id);

		let info = group.info();
		inner.device_groups.insert(device_id.to_owned(), id);
		Some(info)
    }

    pub fn leave(&self, device_id: &str) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        leave_locked(&mut inner, device_id);
    }
	
	pub fn set_new_queue(
		&self,
		device_id: &str,
		item_ids: Vec<Uuid>,
		playing_index: usize,
		position_ticks: i64,
	) -> Result<(Uuid, GroupQueue), &'static str> {
		if item_ids.is_empty() {
			return Err("queue cannot be empty");
		}
		if playing_index >= item_ids.len() {
			return Err("playing index is outside the queue");
		}
		if position_ticks < 0 {
			return Err("start position cannot be negative");
		}

		let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
		let group_id = *inner
			.device_groups
			.get(device_id)
			.ok_or("device is not in a SyncPlay group")?;

		let group = inner
			.groups
			.get_mut(&group_id)
			.ok_or("SyncPlay group no longer exists")?;

		let playlist_item_ids = item_ids.iter().map(|_| Uuid::new_v4()).collect();

		let queue = GroupQueue {
			item_ids,
			playlist_item_ids,
			playing_index,
			position_ticks,
		};
		group.queue = Some(queue.clone());
		group.state = "Waiting".to_owned();
		group.ready_members.clear();
		group.play_started_at = None;


		Ok((group_id, queue))
	}

	pub fn queue_for_device(&self, device_id: &str) -> Option<(Uuid, GroupQueue)> {
		let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
		let group_id = *inner.device_groups.get(device_id)?;
		let queue = inner.groups.get(&group_id)?.queue.clone()?;
		Some((group_id, queue))
	}
	
	pub fn members_for_group(&self, group_id: Uuid) -> Vec<String> {
		let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
		inner
			.groups
			.get(&group_id)
			.map(|group| group.members.iter().cloned().collect())
			.unwrap_or_default()
	}
	
	/// Returns the command information exactly once, when the last member
	/// reports Ready for the currently selected playlist entry.
	pub fn mark_ready(
		&self,
		device_id: &str,
		playlist_item_id: &str,
	) -> Result<Option<(Uuid, Uuid, i64, Vec<String>)>, &'static str> {
		let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
		let group_id = *inner
			.device_groups
			.get(device_id)
			.ok_or("device is not in a SyncPlay group")?;

		let group = inner
			.groups
			.get_mut(&group_id)
			.ok_or("SyncPlay group no longer exists")?;

		if group.state != "Waiting" {
			return Ok(None);
		}

		let queue = group.queue.as_ref().ok_or("group has no queue")?;
		let current_playlist_item_id = queue.playlist_item_ids[queue.playing_index];

		if Uuid::parse_str(playlist_item_id).ok() != Some(current_playlist_item_id) {
			return Err("Ready refers to a different playlist item");
		}

		group.ready_members.insert(device_id.to_owned());

		if group.ready_members.len() != group.members.len() {
			return Ok(None);
		}

		group.state = "Playing".to_owned();
		let members = group.members.iter().cloned().collect();

		Ok(Some((
			group_id,
			current_playlist_item_id,
			queue.position_ticks,
			members,
		)))
	}
	
	pub fn record_play_start(
		&self,
		group_id: Uuid,
		playlist_item_id: Uuid,
		when: chrono::DateTime<chrono::Utc>,
	) -> Result<(), &'static str> {
		let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
		let group = inner.groups.get_mut(&group_id).ok_or("group not found")?;
		let queue = group.queue.as_ref().ok_or("group has no queue")?;

		if group.state != "Playing"
			|| queue.playlist_item_ids[queue.playing_index] != playlist_item_id
		{
			return Err("group is no longer starting that item");
		}

		group.play_started_at = Some(when);
		Ok(())
	}

	pub fn pause(
		&self,
		device_id: &str,
		now: chrono::DateTime<chrono::Utc>,
	) -> Result<(Uuid, Uuid, i64, Vec<String>), &'static str> {
		let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
		let group_id = *inner
			.device_groups
			.get(device_id)
			.ok_or("device is not in a SyncPlay group")?;
		let group = inner.groups.get_mut(&group_id).ok_or("group not found")?;

		if group.state != "Playing" {
			return Err("group is not playing");
		}

		let queue = group.queue.as_mut().ok_or("group has no queue")?;
		let playlist_item_id = queue.playlist_item_ids[queue.playing_index];

		if let Some(started_at) = group.play_started_at {
			let elapsed_ms = (now - started_at).num_milliseconds().max(0);
			queue.position_ticks = queue
				.position_ticks
				.saturating_add(elapsed_ms.saturating_mul(10_000));
		}

		group.play_started_at = None;
		group.state = "Paused".to_owned();
		let members = group.members.iter().cloned().collect();

		Ok((group_id, playlist_item_id, queue.position_ticks, members))
	}

	pub fn unpause(
		&self,
		device_id: &str,
		when: chrono::DateTime<chrono::Utc>,
	) -> Result<(Uuid, Uuid, i64, Vec<String>), &'static str> {
		let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
		let group_id = *inner
			.device_groups
			.get(device_id)
			.ok_or("device is not in a SyncPlay group")?;
		let group = inner.groups.get_mut(&group_id).ok_or("group not found")?;

		if group.state != "Paused" {
			return Err("group is not paused");
		}

		let queue = group.queue.as_ref().ok_or("group has no queue")?;
		let playlist_item_id = queue.playlist_item_ids[queue.playing_index];
		let position_ticks = queue.position_ticks;
		group.play_started_at = Some(when);
		group.state = "Playing".to_owned();
		let members = group.members.iter().cloned().collect();

		Ok((group_id, playlist_item_id, position_ticks, members))
	}

}

fn leave_locked(inner: &mut Inner, device_id: &str) {
    if let Some(id) = inner.device_groups.remove(device_id) {
        if let Some(group) = inner.groups.get_mut(&id) {
            group.members.remove(device_id);
			group.ready_members.remove(device_id);
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
	
	#[test]
	fn only_group_members_can_set_a_valid_queue() {
		let manager = SyncPlayManager::new();
		let group = manager.create("host", "Test".to_owned());
		let episode = Uuid::new_v4();

		assert!(manager.set_new_queue("outsider", vec![episode], 0, 0).is_err());
		assert!(manager.set_new_queue("host", vec![episode], 1, 0).is_err());

		let (group_id, queue) = manager
			.set_new_queue("host", vec![episode], 0, 1_000)
			.unwrap();

		assert_eq!(queue.playlist_item_ids.len(), queue.item_ids.len());
		assert_eq!(manager.get(group_id).unwrap().state, "Waiting");
		assert_eq!(
			manager.members_for_group(group_id),
			vec!["host".to_owned()]
		);

		assert_eq!(group_id, group.group_id);
		assert_eq!(queue.item_ids, vec![episode]);
		assert_eq!(manager.queue_for_device("host"), Some((group_id, queue)));
	}
	
	#[test]
	fn unpause_waits_for_every_member_of_the_current_item() {
		let manager = SyncPlayManager::new();
		let group = manager.create("host", "Test".to_owned());
		manager.join("friend", group.group_id).unwrap();

		let (_, queue) = manager
			.set_new_queue("host", vec![Uuid::new_v4()], 0, 5_000)
			.unwrap();

		let playlist_item_id = queue.playlist_item_ids[0].to_string();

		assert!(manager.mark_ready("outsider", &playlist_item_id).is_err());
		assert!(manager.mark_ready("host", &Uuid::new_v4().to_string()).is_err());

		assert_eq!(manager.mark_ready("host", &playlist_item_id).unwrap(), None);
		assert_eq!(manager.mark_ready("host", &playlist_item_id).unwrap(), None);

		let (group_id, item_id, position, members) = manager
			.mark_ready("friend", &playlist_item_id)
			.unwrap()
			.expect("last member should start the group");

		assert_eq!(group_id, group.group_id);
		assert_eq!(item_id.to_string(), playlist_item_id);
		assert_eq!(position, 5_000);
		assert_eq!(members.len(), 2);
		assert_eq!(manager.get(group_id).unwrap().state, "Playing");
		assert_eq!(manager.mark_ready("host", &playlist_item_id).unwrap(), None);
	}
	
	#[test]
	fn pause_uses_elapsed_playback_position_and_unpause_keeps_it() {
		let manager = SyncPlayManager::new();
		let group = manager.create("host", "Test".to_owned());
		let (_, queue) = manager
			.set_new_queue("host", vec![Uuid::new_v4()], 0, 10_000)
			.unwrap();

		let item_id = queue.playlist_item_ids[0];
		manager.mark_ready("host", &item_id.to_string()).unwrap();

		let start = chrono::Utc::now();
		manager
			.record_play_start(group.group_id, item_id, start)
			.unwrap();

		let paused_at = start + chrono::Duration::seconds(5);
		let (_, _, paused_position, _) = manager.pause("host", paused_at).unwrap();
		assert_eq!(paused_position, 50_010_000);
		assert_eq!(manager.get(group.group_id).unwrap().state, "Paused");

		let resume_at = paused_at + chrono::Duration::seconds(10);
		let (_, _, resumed_position, _) = manager.unpause("host", resume_at).unwrap();
		assert_eq!(resumed_position, paused_position);
		assert_eq!(manager.get(group.group_id).unwrap().state, "Playing");
	}
}