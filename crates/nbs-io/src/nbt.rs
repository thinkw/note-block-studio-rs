//! 读取用的 NBT 解析器。
//!
//! 原版的写侧是 `TAG_*.gml` 那一组脚本，读侧是 `buffer_import` + 手写的循环
//! （`open_schematic.gml` 里直接按字节读）。这里做成一棵显式的标签树，
//! 因为导入和自检都需要「先看结构再取字段」，而不是原版的边读边跳。
//!
//! 只实现原版实际用到的类型；`Long`/`LongArray` 也一并支持，
//! 因为 1.13+ 的结构文件里 `DataVersion` 在别的版本可能是 long。

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    End,
    Byte(i8),
    ByteArray(Vec<u8>),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    String(String),
    /// (元素类型, 元素)；元素类型 0 表示空表。
    List(u8, Vec<Value>),
    /// 按文件里的书写顺序保存，方便核对原版顺序（NBT 也允许重名键）。
    Compound(Vec<(String, Value)>),
}

impl Value {
    pub fn as_i8(&self) -> Option<i8> {
        match self {
            Value::Byte(v) => Some(*v),
            Value::Short(v) => Some(*v as i8),
            Value::Int(v) => Some(*v as i8),
            Value::Long(v) => Some(*v as i8),
            _ => None,
        }
    }
    pub fn as_i16(&self) -> Option<i16> {
        match self {
            Value::Short(v) => Some(*v),
            Value::Byte(v) => Some(*v as i16),
            Value::Int(v) => Some(*v as i16),
            Value::Long(v) => Some(*v as i16),
            _ => None,
        }
    }
    pub fn as_i32(&self) -> Option<i32> {
        match self {
            Value::Int(v) => Some(*v),
            Value::Byte(v) => Some(*v as i32),
            Value::Short(v) => Some(*v as i32),
            Value::Long(v) => Some(*v as i32),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        if let Value::String(s) = self {
            Some(s)
        } else {
            None
        }
    }
    pub fn as_bytes(&self) -> Option<&[u8]> {
        if let Value::ByteArray(b) = self {
            Some(b)
        } else {
            None
        }
    }
    pub fn as_list(&self) -> Option<(u8, &[Value])> {
        if let Value::List(t, items) = self {
            Some((*t, items))
        } else {
            None
        }
    }
    pub fn get(&self, name: &str) -> Option<&Value> {
        if let Value::Compound(m) = self {
            m.iter().find(|(k, _)| k == name).map(|(_, v)| v)
        } else {
            None
        }
    }
    /// compound 的键值，按文件顺序。
    pub fn fields(&self) -> Option<&[(String, Value)]> {
        if let Value::Compound(m) = self {
            Some(m)
        } else {
            None
        }
    }
}

struct Reader<'a> {
    d: &'a [u8],
    p: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self
            .p
            .checked_add(n)
            .ok_or(crate::tr("NBT offset overflow", "NBT 偏移溢出"))?;
        if end > self.d.len() {
            return Err(if crate::lang() == 1 {
                format!("NBT 截断：需要 {n} 字节，只剩 {}", self.d.len() - self.p)
            } else {
                format!(
                    "NBT truncated: {n} bytes needed, {} left",
                    self.d.len() - self.p
                )
            });
        }
        let s = &self.d[self.p..end];
        self.p = end;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn i16(&mut self) -> Result<i16, String> {
        let b = self.take(2)?;
        Ok(i16::from_be_bytes([b[0], b[1]]))
    }
    fn i32(&mut self) -> Result<i32, String> {
        let b = self.take(4)?;
        Ok(i32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn i64(&mut self) -> Result<i64, String> {
        let b = self.take(8)?;
        let a = [b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]];
        Ok(i64::from_be_bytes(a))
    }
    fn f32(&mut self) -> Result<f32, String> {
        let b = self.take(4)?;
        Ok(f32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn f64(&mut self) -> Result<f64, String> {
        let b = self.take(8)?;
        let a = [b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]];
        Ok(f64::from_be_bytes(a))
    }
    fn string(&mut self) -> Result<String, String> {
        let n = self.i16()?.max(0) as usize;
        let b = self.take(n)?;
        Ok(String::from_utf8_lossy(b).into_owned())
    }
    fn named(&mut self) -> Result<(String, Value), String> {
        let id = self.u8()?;
        if id == 0 {
            return Ok((String::new(), Value::End));
        }
        let name = self.string()?;
        let v = self.payload(id)?;
        Ok((name, v))
    }
    fn payload(&mut self, id: u8) -> Result<Value, String> {
        Ok(match id {
            1 => Value::Byte(self.u8()? as i8),
            2 => Value::Short(self.i16()?),
            3 => Value::Int(self.i32()?),
            4 => Value::Long(self.i64()?),
            5 => Value::Float(self.f32()?),
            6 => Value::Double(self.f64()?),
            7 => {
                let n = self.i32()?.max(0) as usize;
                Value::ByteArray(self.take(n)?.to_vec())
            }
            8 => Value::String(self.string()?),
            9 => {
                let ty = self.u8()?;
                let n = self.i32()?.max(0) as usize;
                // 元素数量不能超过剩余字节，否则一个坏文件就能让这里吃掉整块内存。
                if n > self.d.len() - self.p {
                    return Err(if crate::lang() == 1 {
                        format!(
                            "NBT list 声明了 {n} 个元素，但只剩 {}",
                            self.d.len() - self.p
                        )
                    } else {
                        format!(
                            "NBT list declares {n} elements, {} bytes left",
                            self.d.len() - self.p
                        )
                    });
                }
                let mut items = Vec::new();
                for _ in 0..n {
                    items.push(self.payload(ty)?);
                }
                Value::List(ty, items)
            }
            10 => {
                // 递归深度保护：坏文件不应让栈溢出。
                self.compound(0)?
            }
            11 => {
                let n = self.i32()?.max(0) as usize;
                let mut v = Vec::with_capacity(n.min(1024));
                for _ in 0..n {
                    v.push(self.i16()?);
                }
                Value::List(2, v.into_iter().map(Value::Short).collect())
            }
            12 => {
                let n = self.i32()?.max(0) as usize;
                let mut v = Vec::with_capacity(n.min(1024));
                for _ in 0..n {
                    v.push(self.i32()?);
                }
                Value::List(3, v.into_iter().map(Value::Int).collect())
            }
            other => {
                return Err(if crate::lang() == 1 {
                    format!("未知的 NBT 标签类型 {other}")
                } else {
                    format!("unknown NBT tag type {other}")
                })
            }
        })
    }

    fn compound(&mut self, depth: u32) -> Result<Value, String> {
        if depth > 64 {
            return Err("NBT 嵌套过深".into());
        }
        let mut m = Vec::new();
        loop {
            let (k, v) = self.named()?;
            if matches!(v, Value::End) {
                return Ok(Value::Compound(m));
            }
            m.push((k, v));
        }
    }
}

/// 解析未压缩的 NBT 字节流，返回根标签（名字一般为空或 `Schematic`）。
pub fn parse(bytes: &[u8]) -> Result<Value, String> {
    let mut r = Reader { d: bytes, p: 0 };
    let (_, root) = r.named()?;
    Ok(root)
}

/// 带名字一起返回（原版根标签名可能是 `Schematic` 或空串）。
pub fn parse_named(bytes: &[u8]) -> Result<(String, Value), String> {
    let mut r = Reader { d: bytes, p: 0 };
    r.named()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(out: &mut Vec<u8>, v: &Value) {
        match v {
            Value::End => out.push(0),
            Value::Byte(b) => out.push(*b as u8),
            Value::Short(s) => out.extend_from_slice(&s.to_be_bytes()),
            Value::Int(i) => out.extend_from_slice(&i.to_be_bytes()),
            Value::Long(l) => out.extend_from_slice(&l.to_be_bytes()),
            Value::Float(f) => out.extend_from_slice(&f.to_be_bytes()),
            Value::Double(d) => out.extend_from_slice(&d.to_be_bytes()),
            Value::ByteArray(b) => {
                out.extend_from_slice(&(b.len() as i32).to_be_bytes());
                out.extend_from_slice(b);
            }
            Value::String(s) => {
                out.extend_from_slice(&(s.len() as u16).to_be_bytes());
                out.extend_from_slice(s.as_bytes());
            }
            Value::List(t, items) => {
                out.push(*t);
                out.extend_from_slice(&(items.len() as i32).to_be_bytes());
                for i in items {
                    write(out, i);
                }
            }
            Value::Compound(m) => {
                for (k, val) in m {
                    let ty = match val {
                        Value::Byte(_) => 1,
                        Value::Short(_) => 2,
                        Value::Int(_) => 3,
                        Value::Long(_) => 4,
                        Value::Float(_) => 5,
                        Value::Double(_) => 6,
                        Value::ByteArray(_) => 7,
                        Value::String(_) => 8,
                        Value::List(_, _) => 9,
                        Value::Compound(_) => 10,
                        Value::End => 0,
                    };
                    out.push(ty);
                    write(out, &Value::String(k.clone()));
                    write(out, val);
                }
                out.push(0);
            }
        }
    }

    #[test]
    fn roundtrips_the_types_the_exporter_writes() {
        let root = Value::Compound(vec![
            ("Height".to_string(), Value::Short(7)),
            ("Materials".to_string(), Value::String("Alpha".into())),
            ("Blocks".to_string(), Value::ByteArray(vec![0, 25, 93])),
            (
                "TileEntities".to_string(),
                Value::List(
                    10,
                    vec![Value::Compound(vec![
                        ("note".to_string(), Value::Byte(12)),
                        ("x".to_string(), Value::Int(-3)),
                    ])],
                ),
            ),
        ]);
        let mut buf = Vec::new();
        buf.push(10);
        write(&mut buf, &Value::String("Schematic".into()));
        write(&mut buf, &root);

        let back = parse(&buf).unwrap();
        // compound 按写入顺序返回，能用来核对原版的标签顺序。
        let names: Vec<&str> = back
            .fields()
            .unwrap()
            .iter()
            .map(|(k, _)| k.as_str())
            .collect();
        assert_eq!(names, ["Height", "Materials", "Blocks", "TileEntities"]);
        assert_eq!(back.get("Height").and_then(Value::as_i16), Some(7));
        assert_eq!(back.get("Materials").and_then(Value::as_str), Some("Alpha"));
        assert_eq!(
            back.get("Blocks").and_then(Value::as_bytes).unwrap().len(),
            3
        );
        let (ty, items) = back.get("TileEntities").unwrap().as_list().unwrap();
        assert_eq!(ty, 10);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].get("note").and_then(Value::as_i8), Some(12));
        assert_eq!(items[0].get("x").and_then(Value::as_i32), Some(-3));
    }

    #[test]
    fn truncated_and_oversized_input_fail_cleanly() {
        assert!(parse(&[10, 0, 3, b'A', 2, 0]).is_err());
        // list 声明 100 万个元素但后面没数据
        let mut v: Vec<u8> = vec![10, 0, 1, b'L', 9, 3];
        v.extend_from_slice(&1_000_000i32.to_be_bytes());
        v.extend_from_slice(&0i32.to_be_bytes());
        assert!(parse(&v).is_err());
    }
}
