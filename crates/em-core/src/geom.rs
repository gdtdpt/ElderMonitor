//! 矩形与比例几何。
//!
//! 布局引擎（M2）会在这之上做行式打包与候选评分；这里只固化那些
//! 不需要枚举、也没有权衡的运算，保证它们是确定且可单测的。

use serde::{Deserialize, Serialize};

/// 轴对齐矩形。单位与视口一致（物理像素或逻辑像素，由调用方决定，本层不关心）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    /// 从原点铺开的矩形（左上角在 0,0）。
    pub const fn from_size(w: f32, h: f32) -> Self {
        Self::new(0.0, 0.0, w, h)
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub fn area(&self) -> f32 {
        self.w.max(0.0) * self.h.max(0.0)
    }

    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w * 0.5, self.y + self.h * 0.5)
    }

    pub fn is_empty(&self) -> bool {
        self.w <= 0.0 || self.h <= 0.0
    }

    /// 四边各内缩 `d`；收缩过度时宽度/高度夹到 0，不会产生负尺寸。
    pub fn inset(&self, d: f32) -> Self {
        let d = d.max(0.0);
        Self::new(
            self.x + d,
            self.y + d,
            (self.w - d * 2.0).max(0.0),
            (self.h - d * 2.0).max(0.0),
        )
    }

    /// 在 `self` 内取给定宽高比（宽/高）的最大内接矩形，双轴居中。
    ///
    /// 这是「视频不拉伸、只加黑边」的唯一实现点：布局给的是格子矩形，
    /// 视频真实画面由这里算出的矩形决定。
    pub fn fit_aspect(&self, aspect: f32) -> Self {
        if self.is_empty() || !aspect.is_finite() || aspect <= 0.0 {
            return *self;
        }
        let by_width = Self::new(0.0, 0.0, self.w, self.w / aspect);
        let (w, h) = if by_width.h <= self.h {
            (by_width.w, by_width.h)
        } else {
            (self.h * aspect, self.h)
        };
        let w = w.min(self.w);
        let h = h.min(self.h);
        Self::new(
            self.x + (self.w - w) * 0.5,
            self.y + (self.h - h) * 0.5,
            w,
            h,
        )
    }

    /// 从左侧切出宽 `w` 的条，返回 `(左, 余下)`。`w` 会被夹到 `[0, self.w]`。
    pub fn split_left(&self, w: f32) -> (Self, Self) {
        let w = w.clamp(0.0, self.w.max(0.0));
        (
            Self::new(self.x, self.y, w, self.h),
            Self::new(self.x + w, self.y, self.w - w, self.h),
        )
    }

    /// 从右侧切出宽 `w` 的条，返回 `(右, 余下)`。
    pub fn split_right(&self, w: f32) -> (Self, Self) {
        let w = w.clamp(0.0, self.w.max(0.0));
        let rest_w = self.w - w;
        (
            Self::new(self.x + rest_w, self.y, w, self.h),
            Self::new(self.x, self.y, rest_w, self.h),
        )
    }

    /// 从顶部切出高 `h` 的条，返回 `(上, 余下)`。
    pub fn split_top(&self, h: f32) -> (Self, Self) {
        let h = h.clamp(0.0, self.h.max(0.0));
        (
            Self::new(self.x, self.y, self.w, h),
            Self::new(self.x, self.y + h, self.w, self.h - h),
        )
    }

    /// 从底部切出高 `h` 的条，返回 `(下, 余下)`。
    pub fn split_bottom(&self, h: f32) -> (Self, Self) {
        let h = h.clamp(0.0, self.h.max(0.0));
        let rest_h = self.h - h;
        (
            Self::new(self.x, self.y + rest_h, self.w, h),
            Self::new(self.x, self.y, self.w, rest_h),
        )
    }

    /// 水平方向按比例切成 `n` 块等宽矩形（`n = 0` 返回空）。
    pub fn columns(&self, n: usize) -> Vec<Self> {
        if n == 0 {
            return Vec::new();
        }
        let cw = self.w / n as f32;
        (0..n)
            .map(|i| Self::new(self.x + cw * i as f32, self.y, cw, self.h))
            .collect()
    }

    /// 垂直方向按比例切成 `n` 块等高矩形。
    pub fn rows(&self, n: usize) -> Vec<Self> {
        if n == 0 {
            return Vec::new();
        }
        let ch = self.h / n as f32;
        (0..n)
            .map(|i| Self::new(self.x, self.y + ch * i as f32, self.w, ch))
            .collect()
    }
}

/// 宽高比工具：把 mpv 报告的 `width/height` 转成可用比例，非法值退回 16:9。
pub fn aspect_from_size(width: i64, height: i64) -> f32 {
    if width > 0 && height > 0 {
        let a = width as f32 / height as f32;
        if a.is_finite() && (0.1..=10.0).contains(&a) {
            return a;
        }
    }
    crate::config::DEFAULT_ASPECT
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) {
        assert!((a - b).abs() < 1e-3, "expected {b}, got {a}");
    }

    #[test]
    fn fit_aspect_leaves_landscape_alone() {
        let r = Rect::from_size(1920.0, 1080.0);
        let f = r.fit_aspect(16.0 / 9.0);
        approx(f.x, 0.0);
        approx(f.y, 0.0);
        approx(f.w, 1920.0);
        approx(f.h, 1080.0);
    }

    #[test]
    fn fit_aspect_centers_letterbox() {
        // 方形格子里放 16:9 → 上下留黑边，纵向居中
        let f = Rect::from_size(1000.0, 1000.0).fit_aspect(16.0 / 9.0);
        approx(f.w, 1000.0);
        approx(f.h, 562.5);
        approx(f.y, 218.75);
    }

    #[test]
    fn fit_aspect_centers_pillarbox_for_portrait() {
        // 宽格子里放 9:16 → 左右留黑边，横向居中
        let f = Rect::new(0.0, 0.0, 1600.0, 600.0).fit_aspect(9.0 / 16.0);
        approx(f.h, 600.0);
        approx(f.w, 337.5);
        approx(f.x, 631.25);
    }

    #[test]
    fn fit_aspect_rejects_garbage() {
        let r = Rect::from_size(800.0, 600.0);
        assert_eq!(r.fit_aspect(0.0), r);
        assert_eq!(r.fit_aspect(-1.0), r);
        assert_eq!(r.fit_aspect(f32::NAN), r);
    }

    #[test]
    fn splits_are_complementary() {
        let r = Rect::from_size(1920.0, 1080.0);
        let (left, rest) = r.split_left(480.0);
        approx(left.w, 480.0);
        approx(rest.x, 480.0);
        approx(left.w + rest.w, r.w);

        let (top, rest) = r.split_top(270.0);
        approx(top.h, 270.0);
        approx(rest.y, 270.0);
        approx(top.h + rest.h, r.h);

        let (right, rest) = r.split_right(300.0);
        approx(right.x, 1620.0);
        approx(rest.w, 1620.0);

        let (bottom, rest) = r.split_bottom(180.0);
        approx(bottom.y, 900.0);
        approx(rest.h, 900.0);
    }

    #[test]
    fn split_clamps_overflow() {
        let r = Rect::from_size(100.0, 100.0);
        let (left, rest) = r.split_left(500.0);
        approx(left.w, 100.0);
        approx(rest.w, 0.0);
        let (top, rest) = r.split_top(-10.0);
        approx(top.h, 0.0);
        approx(rest.h, 100.0);
    }

    #[test]
    fn columns_and_rows_tile_exactly() {
        let r = Rect::from_size(900.0, 300.0);
        let cols = r.columns(3);
        assert_eq!(cols.len(), 3);
        approx(cols[0].w, 300.0);
        approx(cols[2].x, 600.0);
        let rows = r.rows(2);
        approx(rows[1].y, 150.0);
        assert!(r.columns(0).is_empty());
    }

    #[test]
    fn aspect_from_size_guards() {
        approx(aspect_from_size(1920, 1080), 16.0 / 9.0);
        approx(aspect_from_size(1080, 1920), 9.0 / 16.0);
        approx(aspect_from_size(0, 1080), crate::config::DEFAULT_ASPECT);
        approx(aspect_from_size(10_000, 1), crate::config::DEFAULT_ASPECT);
    }
}
