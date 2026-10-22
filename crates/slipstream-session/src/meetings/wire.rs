use anyhow::{Result, ensure};
use iroh::endpoint::{RecvStream, SendStream};
use serde::{Serialize, de::DeserializeOwned};

pub async fn write<T: Serialize>(send: &mut SendStream, value: &T, limit: usize) -> Result<()> {
    let bytes = postcard::to_stdvec(value)?;
    ensure!(bytes.len() <= limit, "Meeting message is too large");
    send.write_all(&(bytes.len() as u32).to_le_bytes()).await?;
    send.write_all(&bytes).await?;
    Ok(())
}
pub async fn read<T: DeserializeOwned>(recv: &mut RecvStream, limit: usize) -> Result<T> {
    let mut size = [0; 4];
    recv.read_exact(&mut size).await?;
    let size = u32::from_le_bytes(size) as usize;
    ensure!(size <= limit, "Meeting message is too large");
    let mut bytes = vec![0; size];
    recv.read_exact(&mut bytes).await?;
    Ok(postcard::from_bytes(&bytes)?)
}
