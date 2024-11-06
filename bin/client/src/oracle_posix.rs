use kailua_common::oracle::BlobFetchRequest;
use kona_derive::traits::BlobProvider;
use kona_preimage::{HintWriterClient, PreimageKey, PreimageOracleClient};
use std::collections::VecDeque;
use std::fmt::Debug;
use std::io::{BufRead, Read, Write};
use std::mem;
use std::sync::{Arc, Mutex};
use tokio::runtime::Handle;

#[derive(Debug, Clone)]
pub struct POSIXPreimageOracleClient<OR: PreimageOracleClient> {
    pub oracle: Arc<OR>,
    pub key: VecDeque<u8>,
    pub preimage: VecDeque<u8>,
}

// This receives an image from the zkvm to query for a preimage
impl<OR: PreimageOracleClient> Write for POSIXPreimageOracleClient<OR> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.key.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.key.flush()
    }
}

// This writes a preimage to the zkvm
impl<OR: PreimageOracleClient> Read for POSIXPreimageOracleClient<OR> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        // Fetch the preimage if the key length is correct
        if !self.key.is_empty() {
            let key_bytes: [u8; 32] = Vec::from(mem::take(&mut self.key))
                .try_into()
                .or::<std::io::Error>(Err(std::io::ErrorKind::Interrupted.into()))
                .unwrap();
            let key = PreimageKey::try_from(key_bytes).unwrap();
            let preimage = Handle::current()
                .block_on(async { self.oracle.get(key).await })
                .unwrap();
            let preimage_len = (preimage.len() as u64).to_be_bytes();
            self.preimage = [&preimage_len, preimage.as_slice()].concat().into();
        }
        // read the buffered preimage
        self.preimage.read(buf)
    }
}

impl<OR: PreimageOracleClient> BufRead for POSIXPreimageOracleClient<OR> {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        self.preimage.fill_buf()
    }

    fn consume(&mut self, amt: usize) {
        self.preimage.consume(amt)
    }
}

#[derive(Debug, Clone)]
pub struct POSIXCallbackHandle<T> {
    inner: Arc<Mutex<T>>,
}

impl<T> From<T> for POSIXCallbackHandle<T> {
    fn from(value: T) -> Self {
        Self {
            inner: Arc::new(Mutex::new(value)),
        }
    }
}

impl<T: Read> Read for POSIXCallbackHandle<T> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        Ok(self
            .inner
            .lock()
            .unwrap()
            .read(buf)
            .expect("POSIXCallbackHandle::read"))
    }
}

impl<T: Write> Write for POSIXCallbackHandle<T> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        Ok(self
            .inner
            .lock()
            .unwrap()
            .write(buf)
            .expect("POSIXCallbackHandle::write"))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner
            .lock()
            .unwrap()
            .flush()
            .expect("POSIXCallbackHandle::flush");
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct POSIXHintWriterClient<HW: HintWriterClient> {
    pub writer: Arc<HW>,
}

impl<HW: HintWriterClient> Write for POSIXHintWriterClient<HW> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let len = buf.len();
        let string = String::from_utf8(buf.to_vec()).unwrap();
        Handle::current()
            .block_on(async { self.writer.write(&string).await })
            .unwrap();

        Ok(len)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct POSIXBlobProvider<BP: BlobProvider> {
    pub provider: BP,
    pub request: VecDeque<u8>,
    pub blob: VecDeque<u8>,
}

impl<BP: BlobProvider> Write for POSIXBlobProvider<BP> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.request.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.request.flush()
    }
}

impl<BP: BlobProvider> Read for POSIXBlobProvider<BP>
where
    <BP as BlobProvider>::Error: Debug,
{
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        // fetch blob if request is buffered
        if !self.request.is_empty() {
            let request_bytes = Vec::from(mem::take(&mut self.request));
            let request: BlobFetchRequest = bincode::deserialize(&request_bytes).unwrap();
            let blob = Handle::current()
                .block_on(async {
                    self.provider
                        .get_blobs(&request.block_ref, &[request.blob_hash])
                        .await
                })
                .unwrap();
            // todo: refactor kzg commitment logic as function
            let c_kzg_blob = c_kzg::Blob::from_bytes(blob[0].as_slice()).unwrap();
            let settings = alloy::consensus::EnvKzgSettings::default();
            let commitment =
                c_kzg::KzgCommitment::blob_to_kzg_commitment(&c_kzg_blob, settings.get())
                    .expect("Failed to convert blob to commitment");
            let proof = c_kzg::KzgProof::compute_blob_kzg_proof(
                &c_kzg_blob,
                &commitment.to_bytes(),
                settings.get(),
            )
            .unwrap();
            self.blob = [blob[0].as_slice(), commitment.as_slice(), proof.as_slice()]
                .concat()
                .into();
        }
        self.blob.read(buf)
    }
}
