use std::collections::HashMap;
use std::path::PathBuf;

use bitset_fixed::BitSet;
use ndarray::Axis;
use rand::{random, Rng, thread_rng};
use rand::distributions::{Alphanumeric, Standard};
use rand::prelude::*;
use rand::prelude::IteratorRandom;

use fots::types::{
    Field, Flag, FnInfo, GroupId, NumInfo, NumLimit, PtrDir, StrType, TypeId, TypeInfo,
};

use crate::analyze::{Relation, RTable};
use crate::prog::{Arg, ArgIndex, Call, Prog};
use crate::target::Target;
use crate::value::{NumValue, Value};

pub struct Config {
    pub prog_max_len: usize,
    pub str_min_len: usize,
    pub str_max_len: usize,
    pub path_max_depth: usize,
}

pub fn gen(t: &Target, rs: &HashMap<GroupId, RTable>, conf: &Config) -> Prog {
    assert!(!rs.is_empty());

    let mut rng = thread_rng();
    let gid = rs.keys().choose(&mut rng).unwrap();

    let r = &rs[gid];
    let g = &t.groups[gid];
    let seq = choose_seq(r, conf);

    let mut s = State::new(Prog::new(*gid), conf);
    for &i in seq.iter() {
        gen_call(t, &g.fns[i], &mut s);
    }
    s.prog
}

struct State<'a> {
    res: HashMap<TypeId, Vec<ArgIndex>>,
    strs: HashMap<StrType, Vec<String>>,
    prog: Prog,
    conf: &'a Config,
    argi: usize,
}

impl<'a> State<'a> {
    pub fn new(prog: Prog, conf: &'a Config) -> Self {
        Self {
            res: HashMap::new(),
            strs: hashmap! {StrType::FileName => Vec::new()},
            argi: 0,
            prog,
            conf,
        }
    }

    pub fn record_res(&mut self, tid: TypeId) {
        let cid = self.prog.len() - 1;
        let idx = self.res.entry(tid).or_insert(Vec::new());
        idx.push((cid, self.argi))
    }

    pub fn record_str(&mut self, t: StrType, val: &str) {
        let vals = self.strs.entry(t).or_insert(Vec::new());
        vals.push(val.into())
    }

    pub fn add_call(&mut self, call: Call) -> &mut Call {
        self.prog.calls.push(call);
        let len = self.prog.len();
        &mut self.prog.calls[len - 1]
    }
}

fn gen_call(t: &Target, f: &FnInfo, s: &mut State) {
    s.argi = 0;
    s.add_call(Call::new(f.id));
    let call_index = s.prog.calls.len() - 1;
    if f.has_params() {
        for (i, p) in f.iter_param().enumerate() {
            s.argi = i;
            let val = gen_value(p.tid, t, s);
            s.prog.calls[call_index].args.push(Arg { val, tid: p.tid })
        }
    }

    if let Some(tid) = f.r_tid {
        s.argi = s.prog.calls[call_index].args.len();
        if t.is_res(tid) {
            s.record_res(tid);
            s.prog.calls[call_index].ret = Some(Arg {
                tid,
                val: Value::None,
            });
        }
    }
}

fn gen_value(tid: TypeId, t: &Target, s: &mut State) -> Value {
    match t.type_of(tid) {
        TypeInfo::Num(num_info) => gen_num(num_info),
        TypeInfo::Ptr { dir, tid, depth } => {
            assert!(*depth == 1, "Multi-level pointer not supported");
            gen_ptr(*dir, *tid, t, s)
        }
        TypeInfo::Slice { tid, l, h } => gen_slice(*tid, *l, *h, t, s),
        TypeInfo::Str { str_type, vals } => gen_str(str_type, vals, s),
        TypeInfo::Struct { fields, .. } => gen_struct(&fields[..], t, s),
        TypeInfo::Union { fields, .. } => gen_union(&fields[..], t, s),
        TypeInfo::Flag { flags, .. } => gen_flag(&flags[..]),
        TypeInfo::Alias { tid: under_id, .. } => gen_alias(tid, *under_id, t, s),
        TypeInfo::Res { tid: under_tid } => gen_res(tid, *under_tid, t, s),
        TypeInfo::Len {
            tid: _tid,
            path: _p,
            is_param: _is_param,
        } => Value::Num(NumValue::Unsigned(0)),
    }
}

fn gen_alias(tid: TypeId, under_id: TypeId, t: &Target, s: &mut State) -> Value {
    let mut rng = thread_rng();
    if t.is_res(tid) {
        if let Some(res) = s.res.get(&tid) {
            Value::Ref(res.choose(&mut rng).unwrap().clone())
        } else {
            gen_value(under_id, t, s)
        }
    } else {
        gen_value(under_id, t, s)
    }
}

fn gen_res(res_tid: TypeId, tid: TypeId, t: &Target, s: &mut State) -> Value {
    let mut rng = thread_rng();
    if let Some(res) = s.res.get(&res_tid) {
        assert!(!res.is_empty());
        let index = res.choose(&mut rng).unwrap();
        Value::Ref(index.clone())
    } else {
        gen_value(tid, t, s)
    }
}

fn gen_ptr(dir: PtrDir, tid: TypeId, t: &Target, s: &mut State) -> Value {
    if dir != PtrDir::In {
        if t.is_res(tid) {
            s.record_res(tid);
        }
        return Value::default_val(tid, t);
    }
    gen_value(tid, t, s)
}

fn gen_flag(flags: &[Flag]) -> Value {
    assert!(!flags.is_empty());

    let mut rng = thread_rng();

    if rng.gen::<f64>() >= 0.8 {
        Value::Num(NumValue::Signed(rng.gen::<i32>() as i64))
    } else {
        let flag = flags.iter().choose(&mut rng).unwrap();
        let mut val = flag.val;

        loop {
            if rng.gen() {
                let flag = flags.iter().choose(&mut rng).unwrap();
                val &= flag.val;
            } else {
                break;
            }
        }
        Value::Num(NumValue::Signed(val))
    }
}

fn gen_union(fields: &[Field], t: &Target, s: &mut State) -> Value {
    assert!(!fields.is_empty());

    let i = thread_rng().gen_range(0, fields.len());
    let field = &fields[i];

    Value::Opt {
        choice: i,
        val: Box::new(gen_value(field.tid, t, s)),
    }
}

fn gen_struct(fields: &[Field], t: &Target, s: &mut State) -> Value {
    let mut vals = Vec::new();
    for field in fields.iter() {
        vals.push(gen_value(field.tid, t, s));
    }
    Value::Group(vals)
}

fn gen_str(str_type: &StrType, vals: &Option<Vec<String>>, s: &mut State) -> Value {
    let mut rng = thread_rng();
    if let Some(vals) = vals {
        return Value::Str(vals.choose(&mut rng).unwrap().clone());
    }

    let len = rng.gen_range(s.conf.str_min_len, s.conf.str_max_len);
    match str_type {
        StrType::Str => {
            let val = rng
                .sample_iter::<char, Standard>(Standard)
                .take(len)
                .collect::<String>();
            Value::Str(val)
        }
        StrType::CStr => {
            let val = rng.sample_iter(Alphanumeric).take(len).collect::<String>();
            Value::Str(val)
        }
        StrType::FileName => {
            if s.strs[&StrType::FileName].len() != 0 && rng.gen() {
                return Value::Str(s.strs[&StrType::FileName].choose(&mut rng).unwrap().clone());
            }
            let mut path = PathBuf::from(".");
            let mut depth = 0;
            loop {
                let sub_path = rng.sample_iter(Alphanumeric).take(len).collect::<String>();
                path.push(sub_path);
                depth += 1;
                if depth < s.conf.path_max_depth && rng.gen::<f64>() > 0.4 {
                    continue;
                } else {
                    if let Ok(p) = path.into_os_string().into_string() {
                        return Value::Str(p);
                    } else {
                        path = PathBuf::from("/");
                        depth = 0;
                    }
                }
            }
        }
    }
}

fn gen_slice(tid: TypeId, l: isize, h: isize, t: &Target, s: &mut State) -> Value {
    let len: usize = gen_slice_len(l, h);
    let mut vals = Vec::new();

    for _ in 0..len {
        vals.push(gen_value(tid, t, s));
    }
    Value::Group(vals)
}

pub(crate) fn gen_slice_len(l: isize, h: isize) -> usize {
    match (l, h) {
        (-1, -1) => thread_rng().gen_range(0, 8),
        (l, -1) => thread_rng().gen_range(0, l as usize),
        (l, h) => thread_rng().gen_range(l as usize, h as usize),
    }
}

fn gen_num(type_info: &NumInfo) -> Value {
    let mut rng = thread_rng();

    match type_info {
        NumInfo::I8(l) => match l {
            NumLimit::Vals(vals) => {
                Value::Num(NumValue::Signed(*vals.choose(&mut rng).unwrap() as i64))
            }
            NumLimit::Range(r) => {
                Value::Num(NumValue::Signed(rng.gen_range(r.start, r.end) as i64))
            }
            NumLimit::None => Value::Num(NumValue::Signed(rng.gen::<i8>() as i64)),
        },
        NumInfo::I16(l) => match l {
            NumLimit::Vals(vals) => {
                Value::Num(NumValue::Signed(*vals.choose(&mut rng).unwrap() as i64))
            }
            NumLimit::Range(r) => {
                Value::Num(NumValue::Signed(rng.gen_range(r.start, r.end) as i64))
            }
            NumLimit::None => Value::Num(NumValue::Signed(rng.gen::<i16>() as i64)),
        },
        NumInfo::I32(l) => match l {
            NumLimit::Vals(vals) => {
                Value::Num(NumValue::Signed(*vals.choose(&mut rng).unwrap() as i64))
            }
            NumLimit::Range(r) => {
                Value::Num(NumValue::Signed(rng.gen_range(r.start, r.end) as i64))
            }
            NumLimit::None => Value::Num(NumValue::Signed(rng.gen::<i32>() as i64)),
        },
        NumInfo::I64(l) => match l {
            NumLimit::Vals(vals) => {
                Value::Num(NumValue::Signed(*vals.choose(&mut rng).unwrap() as i64))
            }
            NumLimit::Range(r) => {
                Value::Num(NumValue::Signed(rng.gen_range(r.start, r.end) as i64))
            }
            NumLimit::None => Value::Num(NumValue::Signed(rng.gen::<i64>() as i64)),
        },
        NumInfo::U8(l) => match l {
            NumLimit::Vals(vals) => {
                Value::Num(NumValue::Unsigned(*vals.choose(&mut rng).unwrap() as u64))
            }
            NumLimit::Range(r) => {
                Value::Num(NumValue::Unsigned(rng.gen_range(r.start, r.end) as u64))
            }
            NumLimit::None => Value::Num(NumValue::Unsigned(rng.gen::<u8>() as u64)),
        },
        NumInfo::U16(l) => match l {
            NumLimit::Vals(vals) => {
                Value::Num(NumValue::Unsigned(*vals.choose(&mut rng).unwrap() as u64))
            }
            NumLimit::Range(r) => {
                Value::Num(NumValue::Unsigned(rng.gen_range(r.start, r.end) as u64))
            }
            NumLimit::None => Value::Num(NumValue::Unsigned(rng.gen::<u16>() as u64)),
        },
        NumInfo::U32(l) => match l {
            NumLimit::Vals(vals) => {
                Value::Num(NumValue::Unsigned(*vals.choose(&mut rng).unwrap() as u64))
            }
            NumLimit::Range(r) => {
                Value::Num(NumValue::Unsigned(rng.gen_range(r.start, r.end) as u64))
            }
            NumLimit::None => Value::Num(NumValue::Unsigned(rng.gen::<u32>() as u64)),
        },
        NumInfo::U64(l) => match l {
            NumLimit::Vals(vals) => {
                Value::Num(NumValue::Unsigned(*vals.choose(&mut rng).unwrap() as u64))
            }
            NumLimit::Range(r) => {
                Value::Num(NumValue::Unsigned(rng.gen_range(r.start, r.end) as u64))
            }
            NumLimit::None => Value::Num(NumValue::Unsigned(rng.gen::<u64>() as u64)),
        },
        NumInfo::Usize(l) => match l {
            NumLimit::Vals(vals) => {
                Value::Num(NumValue::Unsigned(*vals.choose(&mut rng).unwrap() as u64))
            }
            NumLimit::Range(r) => {
                Value::Num(NumValue::Unsigned(rng.gen_range(r.start, r.end) as u64))
            }
            NumLimit::None => Value::Num(NumValue::Unsigned(rng.gen::<usize>() as u64)),
        },
        NumInfo::Isize(l) => match l {
            NumLimit::Vals(vals) => {
                Value::Num(NumValue::Signed(*vals.choose(&mut rng).unwrap() as i64))
            }
            NumLimit::Range(r) => {
                Value::Num(NumValue::Signed(rng.gen_range(r.start, r.end) as i64))
            }
            NumLimit::None => Value::Num(NumValue::Signed(rng.gen::<isize>() as i64)),
        },
    }
}

fn choose_seq(rs: &RTable, conf: &Config) -> Vec<usize> {
    assert!(rs.len() != 0);

    let mut rng = thread_rng();
    let mut set = BitSet::new(rs.len());
    let mut seq = Vec::new();

    loop {
        let index = rng.gen_range(0, rs.len());
        set.set(index, true);
        seq.push(index);
        let i = seq.len() - 1;
        push_deps(rs, &mut set, &mut seq, i, conf);

        if seq.len() <= conf.prog_max_len && rng.gen() {
            continue;
        } else {
            break;
        }
    }
    seq
}

fn push_deps(rs: &RTable, set: &mut BitSet, seq: &mut Vec<usize>, i: usize, conf: &Config) {
    if i >= seq.len() || seq.len() >= conf.prog_max_len {
        return;
    }
    let index = seq[i];
    let mut deps = Vec::new();
    for (j, r) in rs.index_axis(Axis(0), index).iter().enumerate() {
        if r.eq(&Relation::Some) {
            if !set[j] && random::<f64>() > 0.25 {
                deps.push(j);
                set.set(j, true);
            } else if set[j] && random::<f64>() > 0.75 {
                deps.push(j);
            }
        }

        if r.eq(&Relation::Unknown) {
            if !set[j] && random() {
                deps.push(j);
                set.set(j, true);
            } else if set[j] && random::<f64>() > 0.875 {
                deps.push(j);
            }
        }
    }
    seq.extend(deps);
    push_deps(rs, set, seq, i + 1, conf);
}