use crate::storage::write_atomic;
use ring::{
    aead,
    rand::{SecureRandom, SystemRandom},
};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

pub(super) type KeyRow = (Option<Vec<u8>>, Option<Vec<u8>>);

pub(super) struct EncryptedKeys {
    root: PathBuf,
}

impl EncryptedKeys {
    pub(super) fn new(root: &Path) -> Self {
        Self { root: root.into() }
    }

    fn open(&self) -> Result<Connection, String> {
        let db = Connection::open(self.root.join("credentials.sqlite"))
            .map_err(|e| format!("无法打开内部密钥存储：{e}"))?;
        db.busy_timeout(Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS keys (kind TEXT PRIMARY KEY, nonce BLOB, ciphertext BLOB);",
        )
        .map_err(|e| e.to_string())?;
        Ok(db)
    }

    pub(super) fn stored(&self, kind: &str) -> Result<Option<KeyRow>, String> {
        if !self.root.join("credentials.sqlite").exists() {
            return Ok(None);
        }
        self.open()?
            .query_row(
                "SELECT nonce,ciphertext FROM keys WHERE kind=?1",
                [kind],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|e| e.to_string())
    }

    pub(super) fn decrypt(&self, kind: &str, row: KeyRow) -> Result<Option<String>, String> {
        let (nonce, mut ciphertext) = match row {
            (None, None) => return Ok(None),
            (Some(nonce), Some(ciphertext)) => (nonce, ciphertext),
            _ => return Err("内部加密密钥数据格式错误，请重新填写 API 密钥".into()),
        };
        let nonce: [u8; 12] = nonce.try_into().map_err(|_| "内部密钥随机值格式错误")?;
        let key = self.encryption_key(false)?;
        let aad = format!("AiMemory API key v1:{kind}");
        let plaintext = key
            .open_in_place(
                aead::Nonce::assume_unique_for_key(nonce),
                aead::Aad::from(aad.as_bytes()),
                &mut ciphertext,
            )
            .map_err(|_| "无法解密内部 API 密钥，请重新填写")?;
        String::from_utf8(plaintext.to_vec())
            .map(Some)
            .map_err(|_| "内部 API 密钥编码无效".into())
    }

    pub(super) fn write(&self, kind: &str, value: Option<&str>) -> Result<(), String> {
        let mut db = self.open()?;
        let transaction = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let row = if let Some(value) = value {
            let key = self.encryption_key(true)?;
            let mut nonce = [0u8; 12];
            SystemRandom::new()
                .fill(&mut nonce)
                .map_err(|_| "无法生成加密随机值")?;
            let mut ciphertext = value.as_bytes().to_vec();
            let aad = format!("AiMemory API key v1:{kind}");
            key.seal_in_place_append_tag(
                aead::Nonce::assume_unique_for_key(nonce),
                aead::Aad::from(aad.as_bytes()),
                &mut ciphertext,
            )
            .map_err(|_| "无法加密 API 密钥")?;
            (Some(nonce.to_vec()), Some(ciphertext))
        } else {
            (None, None)
        };
        transaction.execute("INSERT INTO keys VALUES(?1,?2,?3) ON CONFLICT(kind) DO UPDATE SET nonce=excluded.nonce,ciphertext=excluded.ciphertext",params![kind,row.0,row.1]).map_err(|e|e.to_string())?;
        transaction.commit().map_err(|e| e.to_string())
    }

    pub(super) fn restore(&self, kind: &str, row: Option<&KeyRow>) -> Result<(), String> {
        let db = self.open()?;
        match row {
            Some((nonce,ciphertext))=>db.execute("INSERT INTO keys VALUES(?1,?2,?3) ON CONFLICT(kind) DO UPDATE SET nonce=excluded.nonce,ciphertext=excluded.ciphertext",params![kind,nonce,ciphertext]),
            None=>db.execute("DELETE FROM keys WHERE kind=?1",[kind]),
        }.map_err(|e|e.to_string())?;
        Ok(())
    }

    fn encryption_key(&self, create: bool) -> Result<aead::LessSafeKey, String> {
        let path = self.root.join("credentials.key");
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if create && error.kind() == std::io::ErrorKind::NotFound => {
                let mut bytes = vec![0u8; 32];
                SystemRandom::new()
                    .fill(&mut bytes)
                    .map_err(|_| "无法生成内部加密密钥")?;
                write_atomic(&path, &bytes)?;
                bytes
            }
            Err(error) => return Err(format!("无法读取内部加密密钥：{error}")),
        };
        aead::UnboundKey::new(&aead::AES_256_GCM, &bytes)
            .map(aead::LessSafeKey::new)
            .map_err(|_| "内部加密密钥格式错误".into())
    }
}
