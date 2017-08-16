use std::io::{self, Result};

pub trait VarintReader: io::Read {

    fn read_varint(&mut self) -> Result<u64> {

        fn decode<F>(i : u64, mut next : F) -> Result<u64>
        where F : FnMut() -> Result<u64> {
            let v = next()?;

            let r =
                if v & 0x80 == 0x00 {
                    v & 0x7F
                }
                else if v & 0xC0 == 0x80 {
                    (v & 0x3F) << 8 | next()?
                }
                else if (v & 0xF0) == 0xF0 {
                    match v & 0xFC {
                        0xF0 => next()? << 24 | next()? << 16 | next()? << 8 | next()?,
                        0xF4 => next()? << 56 | next()? << 48 | next()? << 40 | next()? << 32 | next()? << 24 | next()? << 16 | next()? << 8 | next()?,
                        0xF8 => !(decode(i, next)?),
                        0xFC => !(v & 0x03),
                        _ => panic!("decoding"),
                    }
                }
                else if (v & 0xF0) == 0xE0 {
                    (v & 0x0F) << 24 | next()? << 16 | next()? << 8 | next()?
                }
                else if (v & 0xE0) == 0xC0 {
                    (v & 0x1F) << 16 | next()? << 8 | next()?
                }
                else {
                    panic!("decoding")
                };
            Ok(r)
        }

        let next = || -> Result<u64> {
            let mut buf = [0u8; 1];
            self.read_exact(&mut buf)?;
            Ok(buf[0] as u64)
        };

        decode(0, next)
    }
}

impl<R: io::Read + ?Sized> VarintReader for R {}

pub trait VarintWriter: io::Write {
    
    fn write_varint(&mut self, value: u64) ->  Result<usize> {
        let mut buf = Vec::<u8>::new();
        let mut i = value;
        if (i & 0x8000000000000000 > 0) && (!i < 0x100000000) {
            // Signed number.
            i = !i;
            if i <= 0x3 {
                // Shortcase for -1 to -4
                buf.push(0xFC | i as u8);
                return self.write(&mut buf[..])
            } else {
                buf.push(0xF8);
            }
        }
        if i < 0x80 {
            // Need top bit clear
            buf.push(i as u8);
        } else if i < 0x4000 {
            // Need top two bits clear
            buf.push(((i >> 8) | 0x80) as u8);
            buf.push((i & 0xFF) as u8);
        } else if i < 0x200000 {
            // Need top three bits clear
            buf.push(((i >> 16) | 0xC0) as u8);
            buf.push(((i >> 8) & 0xFF) as u8);
            buf.push((i & 0xFF) as u8);
        } else if i < 0x10000000 {
            // Need top four bits clear
            buf.push(((i >> 24) | 0xE0) as u8);
            buf.push(((i >> 16) & 0xFF) as u8);
            buf.push(((i >> 8) & 0xFF) as u8);
            buf.push((i & 0xFF) as u8);
        } else if i < 0x100000000 {
            // It's a full 32-bit integer.
            buf.push(0xF0);
            buf.push(((i >> 24) & 0xFF) as u8);
            buf.push(((i >> 16) & 0xFF) as u8);
            buf.push(((i >> 8) & 0xFF) as u8);
            buf.push((i & 0xFF) as u8);
        } else {
            // It's a 64-bit value.
            buf.push(0xF4);
            buf.push(((i >> 56) & 0xFF) as u8);
            buf.push(((i >> 48) & 0xFF) as u8);
            buf.push(((i >> 40) & 0xFF) as u8);
            buf.push(((i >> 32) & 0xFF) as u8);
            buf.push(((i >> 24) & 0xFF) as u8);
            buf.push(((i >> 16) & 0xFF) as u8);
            buf.push(((i >> 8) & 0xFF) as u8);
            buf.push((i & 0xFF) as u8);
        }

        self.write(&mut buf[..])
    }
}

impl<W: io::Write> VarintWriter for W {}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use varint::{VarintReader, VarintWriter};
    use rnd;

    #[test]
    fn random() {
        let mut rnd = rnd::new(1024);
        let m = vec![0xF, 0xFF, 0xFFFF, 0xFFFFFFFF, 0xFFFFFFFFFFFFFFFF];
        for _ in 0..1_000_000 {
            let rnd_num = rnd.next();
            let rnd_num = rnd_num & m[rnd_num as usize % 5];
            let buf = Vec::<u8>::new();
            let mut wtr = Cursor::new(buf);
            wtr.write_varint(rnd_num).unwrap();
            let buf = wtr.into_inner();
            let buf_len = buf.len();
            let mut rdr = Cursor::new(buf);
            let num = rdr.read_varint().unwrap();
            assert_eq!(num, rnd_num);
            println!("{:10} {:20}", buf_len, rnd_num);
        }
    }

    #[test]
    fn limits() {

        let large_uint8  = 0xFF;
        let large_uint16 = 0xFFFF;
        let large_uint32 = 0xFFFFFFFF;
        let large_uint64 = 0xFFFFFFFFFFFFFFFF;

        let small_int8  = -0x7Fi64;
        let small_int16 = -0x7FFFi64;
        let small_int32 = -0x7FFFFFFFi64;
        let small_int64 = -0x7FFFFFFFFFFFFFFFi64;

        let mut buf = vec![0u8; 20];

        {
            let mut wtr = Cursor::new(&mut buf[..]);
            wtr.write_varint(large_uint8).unwrap();
        }
        {
            let mut rdr = Cursor::new(&mut buf[..]);
            let num = rdr.read_varint().unwrap();
            assert_eq!(num, large_uint8);
        }

        {
            let mut wtr = Cursor::new(&mut buf[..]);
            wtr.write_varint(large_uint16).unwrap();
        }
        {
            let mut rdr = Cursor::new(&mut buf[..]);
            let num = rdr.read_varint().unwrap();
            assert_eq!(num, large_uint16);
        }

        {
            let mut wtr = Cursor::new(&mut buf[..]);
            wtr.write_varint(large_uint32).unwrap();
        }
        {
            let mut rdr = Cursor::new(&mut buf[..]);
            let num = rdr.read_varint().unwrap();
            assert_eq!(num, large_uint32);
        }

        {
            let mut wtr = Cursor::new(&mut buf[..]);
            wtr.write_varint(large_uint64).unwrap();
        }
        {
            let mut rdr = Cursor::new(&mut buf[..]);
            let num = rdr.read_varint().unwrap();
            assert_eq!(num, large_uint64);
        }

        {
            let mut wtr = Cursor::new(&mut buf[..]);
            wtr.write_varint(small_int8 as u64).unwrap();
        }
        {
            let mut rdr = Cursor::new(&mut buf[..]);
            let num = rdr.read_varint().unwrap();
            assert_eq!(num as i64, small_int8);
        }

        {
            let mut wtr = Cursor::new(&mut buf[..]);
            wtr.write_varint(small_int16 as u64).unwrap();
        }
        {
            let mut rdr = Cursor::new(&mut buf[..]);
            let num = rdr.read_varint().unwrap();
            assert_eq!(num as i64, small_int16);
        }

        {
            let mut wtr = Cursor::new(&mut buf[..]);
            wtr.write_varint(small_int32 as u64).unwrap();
        }
        {
            let mut rdr = Cursor::new(&mut buf[..]);
            let num = rdr.read_varint().unwrap();
            assert_eq!(num as i64, small_int32);
        }

        {
            let mut wtr = Cursor::new(&mut buf[..]);
            wtr.write_varint(small_int64 as u64).unwrap();
        }
        {
            let mut rdr = Cursor::new(&mut buf[..]);
            let num = rdr.read_varint().unwrap();
            assert_eq!(num as i64, small_int64);
        }
    }
}
