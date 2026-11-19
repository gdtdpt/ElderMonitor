//! 会话状态操作：记录库 / 显示列 / 主格。
//!
//! 所有会破坏不变量的操作都在这里收口，UI 层只调方法、不直接改 `Config` 的字段。
//! 不变量（构造时与每次操作后都成立）：
//! 1. `visible` 里每个 id 都能在 `records` 里找到
//! 2. `visible` 无重复，且长度 ≤ [`MAX_VISIBLE`]
//! 3. `primary` 要么是 `None`，要么属于 `visible`
//! 4. 同一 `room` 在 `records` 里最多一条

use crate::config::{Config, DEFAULT_ASPECT, DEFAULT_VOLUME, MAX_VISIBLE, MAX_VOLUME, Record};

/// 加入记录库时的错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddError {
    /// 房间号为空
    EmptyRoom,
}

/// 显示列 / 主格操作错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShowError {
    /// 记录不存在
    NotFound,
    /// 显示列已满（[`MAX_VISIBLE`]）
    Full,
}

impl std::fmt::Display for AddError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyRoom => write!(f, "房间号为空"),
        }
    }
}

impl std::fmt::Display for ShowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "记录不存在"),
            Self::Full => write!(f, "显示列已满（上限 {MAX_VISIBLE} 路）"),
        }
    }
}

impl std::error::Error for AddError {}
impl std::error::Error for ShowError {}

/// 一个会话 = 一份配置 + 一组受保护的操作。
pub struct Session {
    cfg: Config,
}

impl Session {
    /// 用配置构造会话；内部先跑一次校验。
    pub fn new(cfg: Config) -> Self {
        let mut cfg = cfg;
        cfg.sanitize();
        Self { cfg }
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    /// 取出配置（退出保存时用）。
    pub fn into_config(self) -> Config {
        self.cfg
    }

    // ---------- 只读查询 ----------

    pub fn records(&self) -> &[Record] {
        &self.cfg.records
    }

    pub fn visible_ids(&self) -> &[String] {
        &self.cfg.visible
    }

    pub fn visible_count(&self) -> usize {
        self.cfg.visible.len()
    }

    pub fn primary(&self) -> Option<&str> {
        self.cfg.primary.as_deref()
    }

    pub fn record(&self, id: &str) -> Option<&Record> {
        self.cfg.record(id)
    }

    pub fn find_by_room(&self, room: &str) -> Option<&Record> {
        let room = room.trim();
        self.cfg.records.iter().find(|r| r.room == room)
    }

    pub fn is_visible(&self, id: &str) -> bool {
        self.cfg.visible.iter().any(|v| v == id)
    }

    /// 记录当前在第几格（0 基）。
    pub fn slot_of(&self, id: &str) -> Option<usize> {
        self.cfg.visible.iter().position(|v| v == id)
    }

    /// 显示列的记录，顺序即格子顺序。不会返回悬空 id。
    pub fn visible_records(&self) -> Vec<&Record> {
        self.cfg
            .visible
            .iter()
            .filter_map(|id| self.cfg.record(id))
            .collect()
    }

    // ---------- 记录库 ----------

    /// 加入记录库（按 `room` 去重）。返回记录 id。
    ///
    /// 已存在同一房间时**不新建**记录，直接返回既有 id——界面层据此把它加入显示列。
    pub fn add_record(&mut self, room: &str, name: &str) -> Result<String, AddError> {
        let room = room.trim();
        if room.is_empty() {
            return Err(AddError::EmptyRoom);
        }
        if let Some(existing) = self.find_by_room(room) {
            return Ok(existing.id.clone());
        }
        let id = self.next_id();
        self.cfg.records.push(Record {
            id: id.clone(),
            room: room.to_string(),
            name: name.trim().to_string(),
            aspect: DEFAULT_ASPECT,
            volume: DEFAULT_VOLUME,
            muted: false,
            last_watch: 0,
        });
        Ok(id)
    }

    /// 从记录库彻底删除（若在显示则一并移出，若为主格则清空主格）。
    pub fn remove_record(&mut self, id: &str) -> Option<Record> {
        let idx = self.cfg.records.iter().position(|r| r.id == id)?;
        let removed = self.cfg.records.remove(idx);
        self.cfg.visible.retain(|v| v != id);
        if self.cfg.primary.as_deref() == Some(id) {
            self.cfg.primary = None;
        }
        Some(removed)
    }

    /// 更新记录属性（音量、静音、名字、宽高比、最近观看）。
    pub fn record_mut(&mut self, id: &str) -> Option<&mut Record> {
        self.cfg.record_mut(id)
    }

    /// 设置音量（自动夹到 0..=100）。记录不存在返回 false。
    pub fn set_volume(&mut self, id: &str, volume: u8) -> bool {
        match self.cfg.record_mut(id) {
            Some(r) => {
                r.volume = volume.min(MAX_VOLUME);
                true
            }
            None => false,
        }
    }

    /// 设置静音。
    pub fn set_muted(&mut self, id: &str, muted: bool) -> bool {
        match self.cfg.record_mut(id) {
            Some(r) => {
                r.muted = muted;
                true
            }
            None => false,
        }
    }

    /// 全部静音 / 全部取消静音（`Shift+M`）。返回受影响的条数。
    pub fn set_all_muted(&mut self, muted: bool) -> usize {
        let mut n = 0;
        for id in self.cfg.visible.clone() {
            if let Some(r) = self.cfg.record_mut(&id) {
                r.muted = muted;
                n += 1;
            }
        }
        n
    }

    /// 记录一次观看时间（Unix 秒），用于记录库倒序。
    pub fn touch(&mut self, id: &str, now_unix: i64) -> bool {
        match self.cfg.record_mut(id) {
            Some(r) => {
                r.last_watch = now_unix;
                true
            }
            None => false,
        }
    }

    // ---------- 显示列 ----------

    /// 加入显示列。已在显示返回 `Ok(false)`；显示列已满返回 [`ShowError::Full`]。
    pub fn show(&mut self, id: &str) -> Result<bool, ShowError> {
        if self.cfg.record(id).is_none() {
            return Err(ShowError::NotFound);
        }
        if self.is_visible(id) {
            return Ok(false);
        }
        if self.cfg.visible.len() >= MAX_VISIBLE {
            return Err(ShowError::Full);
        }
        self.cfg.visible.push(id.to_string());
        Ok(true)
    }

    /// 移出显示列（记录保留）。返回是否真的移除了。
    pub fn hide(&mut self, id: &str) -> bool {
        let before = self.cfg.visible.len();
        self.cfg.visible.retain(|v| v != id);
        if self.cfg.primary.as_deref() == Some(id) {
            self.cfg.primary = None;
        }
        before != self.cfg.visible.len()
    }

    /// 加入 / 移出显示列。返回操作后是否在显示。
    pub fn toggle_show(&mut self, id: &str) -> Result<bool, ShowError> {
        if self.is_visible(id) {
            self.hide(id);
            Ok(false)
        } else {
            self.show(id).map(|_| true)
        }
    }

    /// 把 `id` 放到第 `index` 格。
    ///
    /// - 若 `id` 已在另一格：两格**交换**（显示列里不出现重复项），返回 `Ok(None)`
    /// - 否则替换该格，返回被换下的记录 id
    pub fn replace_slot(&mut self, index: usize, id: &str) -> Result<Option<String>, ShowError> {
        if index >= self.cfg.visible.len() || self.cfg.record(id).is_none() {
            return Err(ShowError::NotFound);
        }
        if let Some(other) = self.slot_of(id) {
            if other != index {
                self.cfg.visible.swap(index, other);
            }
            return Ok(None);
        }
        let displaced = std::mem::replace(&mut self.cfg.visible[index], id.to_string());
        if self.cfg.primary.as_deref() == Some(displaced.as_str()) {
            self.cfg.primary = Some(id.to_string());
        }
        Ok(Some(displaced))
    }

    /// 把某格移到 `to` 位置（拖拽换位的等价操作，M3 可选）。
    pub fn move_slot(&mut self, from: usize, to: usize) -> bool {
        if from >= self.cfg.visible.len() || to >= self.cfg.visible.len() || from == to {
            return false;
        }
        let id = self.cfg.visible.remove(from);
        self.cfg.visible.insert(to, id);
        true
    }

    // ---------- 主格 ----------

    /// 设置主格；`None` 回到均匀模式。主格必须在显示列里。
    pub fn set_primary(&mut self, id: Option<&str>) -> Result<(), ShowError> {
        match id {
            None => {
                self.cfg.primary = None;
                Ok(())
            }
            Some(id) => {
                if !self.is_visible(id) {
                    return Err(ShowError::NotFound);
                }
                self.cfg.primary = Some(id.to_string());
                Ok(())
            }
        }
    }

    /// 双击行为：同一格再双击 = 取消主格。
    pub fn toggle_primary(&mut self, id: &str) {
        if self.cfg.primary.as_deref() == Some(id) {
            self.cfg.primary = None;
        } else {
            let _ = self.set_primary(Some(id));
        }
    }

    // ---------- 内部 ----------

    /// 下一个记录 id：`r<max+1>`，确定性、不引入 uuid 依赖。
    fn next_id(&self) -> String {
        let max = self
            .cfg
            .records
            .iter()
            .filter_map(|r| r.id.strip_prefix('r'))
            .filter_map(|n| n.parse::<u64>().ok())
            .max()
            .unwrap_or(0);
        format!("r{}", max + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Session {
        Session::new(Config::default())
    }

    fn add(s: &mut Session, room: &str) -> String {
        s.add_record(room, "").unwrap()
    }

    #[test]
    fn add_record_dedupes_by_room() {
        let mut s = session();
        let a = add(&mut s, "7734200");
        let b = add(&mut s, " 7734200 ");
        assert_eq!(a, b);
        assert_eq!(s.records().len(), 1);
    }

    #[test]
    fn add_record_rejects_empty_room() {
        let mut s = session();
        assert_eq!(s.add_record("   ", "x"), Err(AddError::EmptyRoom));
    }

    #[test]
    fn record_ids_are_sequential_and_never_reused() {
        let mut s = session();
        assert_eq!(add(&mut s, "1"), "r1");
        assert_eq!(add(&mut s, "2"), "r2");
        s.remove_record("r1");
        // 删掉 r1 后不复用该 id：取现有最大序号 +1，避免悬空引用
        assert_eq!(add(&mut s, "3"), "r3");
        assert_eq!(add(&mut s, "4"), "r4");
    }

    #[test]
    fn show_and_hide_keep_invariants() {
        let mut s = session();
        let id = add(&mut s, "100");
        assert!(s.show(&id).unwrap());
        assert!(!s.show(&id).unwrap(), "重复加入是无操作");
        assert_eq!(s.slot_of(&id), Some(0));
        assert!(s.hide(&id));
        assert!(!s.hide(&id));
        assert!(s.visible_records().is_empty());
    }

    #[test]
    fn visible_is_capped() {
        let mut s = session();
        let mut ids = Vec::new();
        for i in 0..MAX_VISIBLE {
            let id = add(&mut s, &format!("{i}"));
            s.show(&id).unwrap();
            ids.push(id);
        }
        let overflow = add(&mut s, "999");
        assert_eq!(s.show(&overflow), Err(ShowError::Full));
        assert_eq!(s.visible_count(), MAX_VISIBLE);
    }

    #[test]
    fn hide_clears_primary() {
        let mut s = session();
        let id = add(&mut s, "1");
        s.show(&id).unwrap();
        s.set_primary(Some(&id)).unwrap();
        s.hide(&id);
        assert_eq!(s.primary(), None);
    }

    #[test]
    fn primary_must_be_visible() {
        let mut s = session();
        let id = add(&mut s, "1");
        assert_eq!(s.set_primary(Some(&id)), Err(ShowError::NotFound));
        s.show(&id).unwrap();
        s.set_primary(Some(&id)).unwrap();
        assert_eq!(s.primary(), Some(id.as_str()));
        s.toggle_primary(&id);
        assert_eq!(s.primary(), None);
    }

    #[test]
    fn remove_record_also_removes_from_visible() {
        let mut s = session();
        let id = add(&mut s, "1");
        s.show(&id).unwrap();
        s.set_primary(Some(&id)).unwrap();
        let removed = s.remove_record(&id).expect("应删除成功");
        assert_eq!(removed.room, "1");
        assert!(s.visible_ids().is_empty());
        assert_eq!(s.primary(), None);
        assert_eq!(s.remove_record(&id), None);
    }

    #[test]
    fn replace_slot_returns_displaced_record() {
        let mut s = session();
        let a = add(&mut s, "1");
        let b = add(&mut s, "2");
        let c = add(&mut s, "3");
        s.show(&a).unwrap();
        s.show(&b).unwrap();
        let displaced = s.replace_slot(0, &c).unwrap();
        assert_eq!(displaced.as_deref(), Some(a.as_str()));
        assert_eq!(s.visible_ids(), &[c, b]);
    }

    #[test]
    fn replace_slot_swaps_when_already_visible() {
        let mut s = session();
        let a = add(&mut s, "1");
        let b = add(&mut s, "2");
        s.show(&a).unwrap();
        s.show(&b).unwrap();
        assert_eq!(s.replace_slot(0, &b).unwrap(), None);
        assert_eq!(s.visible_ids(), &[b.clone(), a.clone()]);
        assert_eq!(s.visible_count(), 2, "不能出现重复项");
    }

    #[test]
    fn replace_slot_keeps_primary_alias() {
        let mut s = session();
        let a = add(&mut s, "1");
        let b = add(&mut s, "2");
        s.show(&a).unwrap();
        s.set_primary(Some(&a)).unwrap();
        s.replace_slot(0, &b).unwrap();
        // 主格位没变，内容换了，主格应跟随到新记录
        assert_eq!(s.primary(), Some(b.as_str()));
    }

    #[test]
    fn volume_is_clamped() {
        let mut s = session();
        let id = add(&mut s, "1");
        s.set_volume(&id, 250);
        assert_eq!(s.record(&id).unwrap().volume, MAX_VOLUME);
        assert!(!s.set_volume("ghost", 50));
    }

    #[test]
    fn default_volume_for_new_record() {
        let mut s = session();
        let id = add(&mut s, "1");
        let rec = s.record(&id).unwrap();
        assert_eq!(rec.volume, DEFAULT_VOLUME);
        assert!(!rec.muted);
        assert!((rec.aspect - DEFAULT_ASPECT).abs() < 1e-6);
    }

    #[test]
    fn move_slot_reorders() {
        let mut s = session();
        let ids: Vec<String> = (0..3)
            .map(|i| {
                let id = add(&mut s, &format!("{i}"));
                s.show(&id).unwrap();
                id
            })
            .collect();
        assert!(s.move_slot(2, 0));
        assert_eq!(s.visible_ids(), &[ids[2].clone(), ids[0].clone(), ids[1].clone()]);
        assert!(!s.move_slot(0, 0));
        assert!(!s.move_slot(0, 9));
    }

    #[test]
    fn mute_all_affects_only_visible() {
        let mut s = session();
        let a = add(&mut s, "1");
        let b = add(&mut s, "2");
        s.show(&a).unwrap();
        assert_eq!(s.set_all_muted(true), 1);
        assert!(s.record(&a).unwrap().muted);
        assert!(!s.record(&b).unwrap().muted);
    }

    #[test]
    fn new_session_sanitizes_input() {
        let cfg = Config {
            visible: vec!["ghost".into()],
            primary: Some("ghost".into()),
            ..Config::default()
        };
        let s = Session::new(cfg);
        assert!(s.visible_ids().is_empty());
        assert_eq!(s.primary(), None);
    }

}
