//! 特效的图元层。
//!
//! 特效只产出这一层的 [`Frame`]，不直接碰 `Painter`：底座按 z 序把各特效的一帧
//! 合成起来，再统一落笔。这样「叠加在编辑器上」和「独占整屏舞台」两种表面共用
//! 同一条绘制路径，以后接插件 ABI 时边界也是这一层，而不是 egui 的绘制调用。
//!
//! 原版 Synthesia 那种光晕是加法混合，本项目的 egui/epaint 0.31 没有
//! `CompositeMode`，所以 [`Primitive::Glow`] 用同心圆角描边叠几层近似，
//! 观感接近、代价是只有 alpha 合成。
//!
//! 音符本体是几何体（圆角矩形），方块是贴图（[`ImagePrim`]）——后者带
//! `Arc<TextureHandle>`，所以 `Frame` 不是 `Copy`，每帧复用同一个实例、`clear()` 一下。

use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, TextureHandle};
use std::sync::Arc;

/// 单个几何图元。保持 Copy，一帧几千个也不该有堆分配。
#[derive(Clone, Copy, Debug)]
pub enum Primitive {
    /// 矩形：胶囊形音符就是 `rounding = 宽/2` 的矩形。
    Rect {
        rect: Rect,
        fill: Color32,
        stroke: Option<(f32, Color32)>,
        rounding: f32,
    },
    /// 近似加法混合的光晕：从 `rect` 向外扩 `spread` 像素叠若干层描边。
    Glow {
        rect: Rect,
        spread: f32,
        color: Color32,
    },
    /// 线段：分隔线、扫描线、琴键投影线。
    Line {
        from: Pos2,
        to: Pos2,
        width: f32,
        color: Color32,
    },
}

/// 贴图图元：方块 tile 这类「有素材」的东西。
/// 没有 `Debug`：`TextureHandle` 自己就不可调试，硬做也只能打印个名字。
#[derive(Clone)]
pub struct ImagePrim {
    pub rect: Rect,
    pub uv: Rect,
    pub tint: Color32,
    pub texture: Arc<TextureHandle>,
}

/// 一帧图元的上限：再多就该怀疑是哪个特效没做窗口剔除了。超出时底座截断，
/// 而不是让界面卡死。
pub const MAX_PRIMITIVES: usize = 6000;

/// 光晕的同心描边层数。4 层足够把硬边糊开，又不会让一屏光晕变成几千个矩形。
const GLOW_RINGS: usize = 4;

/// 一帧要画的东西。
#[derive(Default, Clone)]
pub struct Frame {
    pub prims: Vec<Primitive>,
    pub images: Vec<ImagePrim>,
}

impl Frame {
    pub fn clear(&mut self) {
        self.prims.clear();
        self.images.clear();
    }

    pub fn len(&self) -> usize {
        self.prims.len() + self.images.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn rect(&mut self, rect: Rect, fill: Color32, rounding: f32) {
        self.prims.push(Primitive::Rect {
            rect,
            fill,
            stroke: None,
            rounding,
        });
    }

    pub fn outlined(&mut self, rect: Rect, fill: Color32, stroke: (f32, Color32), rounding: f32) {
        self.prims.push(Primitive::Rect {
            rect,
            fill,
            stroke: Some(stroke),
            rounding,
        });
    }

    pub fn glow(&mut self, rect: Rect, spread: f32, color: Color32) {
        self.prims.push(Primitive::Glow {
            rect,
            spread,
            color,
        });
    }

    pub fn line(&mut self, from: Pos2, to: Pos2, width: f32, color: Color32) {
        self.prims.push(Primitive::Line {
            from,
            to,
            width,
            color,
        });
    }

    pub fn image(&mut self, rect: Rect, texture: Arc<TextureHandle>, tint: Color32) {
        self.images.push(ImagePrim {
            rect,
            uv: Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            tint,
            texture,
        });
    }

    /// 把两个特效的产出叠在一起（z 小的在前，后画的压在上面）。
    pub fn extend(&mut self, other: &Frame) {
        self.prims.extend(other.prims.iter().copied());
        self.images.extend(other.images.iter().cloned());
    }

    /// 超上限时截断，顺带把「谁没做剔除」暴露出来：调用方（`FxStack`）记一句提示。
    pub fn truncate(&mut self) -> bool {
        let over = self.len() > MAX_PRIMITIVES;
        if over {
            self.prims.truncate(MAX_PRIMITIVES);
            self.images.clear();
        }
        over
    }
}

fn dim(color: Color32, alpha_scale: f32) -> Color32 {
    if alpha_scale >= 1.0 {
        return color;
    }
    Color32::from_rgba_premultiplied(
        color.r(),
        color.g(),
        color.b(),
        (color.a() as f32 * alpha_scale.clamp(0.0, 1.0)) as u8,
    )
}

/// 把一帧画到 `p` 上。`alpha_scale` 是叠加到已有界面上时的整体透明度
/// （舞台表面传 1，叠加表面按特效自己的 `opacity` 传）。
pub fn paint(p: &Painter, frame: &Frame, alpha_scale: f32) {
    for prim in &frame.prims {
        match *prim {
            Primitive::Rect {
                rect,
                fill,
                stroke,
                rounding,
            } => match stroke {
                Some((width, color)) => {
                    p.rect_stroke(
                        rect,
                        rounding.max(0.0),
                        Stroke::new(width, dim(color, alpha_scale)),
                        StrokeKind::Middle,
                    );
                }
                None => {
                    p.rect_filled(rect, rounding.max(0.0), dim(fill, alpha_scale));
                }
            },
            Primitive::Glow {
                rect,
                spread,
                color,
            } => {
                let radius = rect.width().min(rect.height()) / 2.0;
                for ring in 1..=GLOW_RINGS {
                    let t = ring as f32 / GLOW_RINGS as f32;
                    let grow = spread * t;
                    // 越外圈越淡，衰减取平方：线性衰减看起来像一圈圈台阶。
                    let a = ((1.0 - t) * (1.0 - t) * color.a() as f32 * alpha_scale) as u8;
                    p.rect_stroke(
                        rect.expand(grow),
                        radius + grow,
                        Stroke::new(
                            (spread / GLOW_RINGS as f32).max(1.0),
                            Color32::from_rgba_premultiplied(color.r(), color.g(), color.b(), a),
                        ),
                        StrokeKind::Inside,
                    );
                }
            }
            Primitive::Line {
                from,
                to,
                width,
                color,
            } => {
                p.line_segment([from, to], Stroke::new(width, dim(color, alpha_scale)));
            }
        }
    }
    for img in &frame.images {
        p.add(Shape::image(
            img.texture.id(),
            img.rect,
            img.uv,
            dim(img.tint, alpha_scale),
        ));
    }
}

/// 供单测与「特效自检」使用：只数图元、不落笔。
pub fn into_shapes(frame: &Frame) -> Vec<Shape> {
    frame
        .prims
        .iter()
        .map(|prim| match *prim {
            Primitive::Rect {
                rect,
                fill,
                stroke,
                rounding,
            } => match stroke {
                Some((width, color)) => Shape::rect_stroke(
                    rect,
                    rounding.max(0.0),
                    Stroke::new(width, color),
                    StrokeKind::Middle,
                ),
                None => Shape::rect_filled(rect, rounding.max(0.0), fill),
            },
            Primitive::Glow { rect, spread, .. } => Shape::rect_filled(
                rect.expand(spread),
                rect.width().min(rect.height()) / 2.0 + spread,
                Color32::TRANSPARENT,
            ),
            Primitive::Line {
                from,
                to,
                width,
                color,
            } => Shape::line_segment([from, to], Stroke::new(width, color)),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Vec2;

    fn rect() -> Rect {
        Rect::from_min_size(Pos2::new(10.0, 10.0), Vec2::new(40.0, 20.0))
    }

    #[test]
    fn frame_collects_both_channels() {
        let mut f = Frame::default();
        f.rect(rect(), Color32::RED, 4.0);
        f.line(Pos2::ZERO, Pos2::new(1.0, 1.0), 2.0, Color32::WHITE);
        assert_eq!(f.len(), 2);
        assert_eq!(into_shapes(&f).len(), 2);
        f.clear();
        assert!(f.is_empty());
    }

    #[test]
    fn extending_keeps_paint_order() {
        let mut a = Frame::default();
        let mut b = Frame::default();
        a.rect(rect(), Color32::RED, 0.0);
        b.rect(rect().expand(1.0), Color32::BLUE, 0.0);
        a.extend(&b);
        assert_eq!(a.len(), 2);
        // 后一个特效的图元排在后面，即压在上面。
        let Primitive::Rect { fill, .. } = a.prims[1] else {
            panic!("图元类型变了");
        };
        assert_eq!(fill, Color32::BLUE);
    }

    #[test]
    fn oversized_frame_is_truncated_not_drawn() {
        let mut f = Frame::default();
        for _ in 0..MAX_PRIMITIVES + 10 {
            f.rect(rect(), Color32::RED, 0.0);
        }
        assert!(f.truncate(), "超上限要报出来，别让「没剔除」无声地卡住界面");
        assert_eq!(f.len(), MAX_PRIMITIVES);
        let mut ok = Frame::default();
        ok.rect(rect(), Color32::RED, 0.0);
        assert!(!ok.truncate());
    }

    #[test]
    fn dimming_only_touches_alpha() {
        assert_eq!(dim(Color32::WHITE, 1.0), Color32::WHITE);
        let half = dim(Color32::from_rgb(10, 20, 30), 0.5);
        assert_eq!([half.r(), half.g(), half.b()], [10, 20, 30]);
        assert_eq!(half.a(), 127);
        // 特效把整体透明度调到 0 时不能留下残影。
        assert_eq!(dim(Color32::WHITE, 0.0).a(), 0);
    }
}
