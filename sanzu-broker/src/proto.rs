use byteorder::ByteOrder;
use byteorder::{LittleEndian, ReadBytesExt};
use std::io::{Read, Write};
use std::{io, io::Cursor};
pub use sanzu_tunnel::tunnel;

const MAX_PACKET_LEN: usize = 10 * 1024 * 1024; // 10 Mo

/// Read + Write trait used to send protobuf serialized messages
pub trait ReadWrite: Read + Write {}

pub struct Tunnel {}

impl Tunnel {
    pub fn new() -> Self {
        Tunnel {}
    }

    pub fn send<T>(&mut self, stream: &mut dyn Write, req: T) -> io::Result<()>
    where
        T: prost::Message,
    {
        // Send length, on 8 bytes
        let mut buffer = vec![0u8; 8];
        LittleEndian::write_u64(&mut buffer, req.encoded_len() as u64);
        // Encode request
        let mut req_buf = vec![];
        req.encode(&mut req_buf)?;
        buffer.append(&mut req_buf);
        // Send request
        stream.write_all(&buffer)?;
        Ok(())
    }

    pub fn recv<T>(&mut self, stream: &mut dyn Read) -> io::Result<T>
    where
        T: prost::Message + Default,
    {
        let mut buffer = vec![0u8; 0x8];
        stream.read_exact(&mut buffer)?;

        let mut rdr = Cursor::new(buffer);
        let len = ReadBytesExt::read_u64::<LittleEndian>(&mut rdr)? as usize;
        if len > MAX_PACKET_LEN {
            return Err(io::Error::new(io::ErrorKind::Other, "Packet too big!"));
        }
        let mut req_buffer = vec![0u8; len];
        stream.read_exact(&mut req_buffer)?;
        Ok(prost::Message::decode(req_buffer.as_slice())?)
    }
}
