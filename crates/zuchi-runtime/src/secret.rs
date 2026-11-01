//! 記憶の鍵。OS の鍵置き場に置き、ファイルには書かない。

use keyring::Entry;
use zuchi_core::ports::Result;
use zuchi_memory::MemoryKey;

const SERVICE: &str = "dev.inouedaiki.ai-zuchi";
const ACCOUNT: &str = "memory-key";

pub fn load_or_create_key() -> Result<MemoryKey> {
    let entry = Entry::new(SERVICE, ACCOUNT)?;
    match entry.get_secret() {
        Ok(secret) => {
            let bytes: [u8; 32] = secret
                .try_into()
                .map_err(|_| "鍵置き場の鍵の長さが正しくない")?;
            Ok(MemoryKey::from_bytes(bytes))
        }
        Err(keyring::Error::NoEntry) => {
            let mut bytes = [0u8; 32];
            getrandom::fill(&mut bytes).map_err(|e| e.to_string())?;
            entry.set_secret(&bytes)?;
            Ok(MemoryKey::from_bytes(bytes))
        }
        Err(error) => Err(error.into()),
    }
}
