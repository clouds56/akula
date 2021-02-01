use super::*;
use crate::{common, dbutils, CursorDupSort};
use async_stream::try_stream;
use bytes::{BufMut, Bytes, BytesMut};
use std::io::Write;

pub fn walk<
    'cur,
    C: CursorDupSort,
    Key: Send + Unpin + 'cur,
    Decoder: Fn(Bytes, Bytes) -> (u64, Key, Bytes) + 'cur,
>(
    c: &'cur mut C,
    decoder: Decoder,
    from: u64,
    to: u64,
) -> impl Stream<Item = anyhow::Result<(u64, Key, Bytes)>> + '_ {
    try_stream! {
        let (mut k, mut v) = c.seek(&dbutils::encode_block_number(from)).await?;
        loop {
            if k.is_empty() {
                break;
            }

            let (block_num, k1, v1) = (decoder)(k, v);
            if block_num > to {
                break;
            }

            yield (block_num, k1, v1);

            (k, v) = c.next().await?
        }
    }
}

pub async fn find_in_storage_changeset_2<C: CursorDupSort>(
    c: &mut C,
    block_number: u64,
    key_prefix_len: usize,
    k: &[u8],
) -> anyhow::Result<Option<Bytes>> {
    do_search_2(
        c,
        block_number,
        key_prefix_len,
        &k[..key_prefix_len],
        &k[key_prefix_len + common::INCARNATION_LENGTH
            ..key_prefix_len + common::HASH_LENGTH + common::INCARNATION_LENGTH],
        u64::from_be_bytes(*array_ref!(&k[key_prefix_len..], 0, 8)),
    )
    .await
}

pub async fn find_without_incarnation_in_storage_changeset_2<C: CursorDupSort>(
    c: &mut C,
    block_number: u64,
    key_prefix_len: usize,
    addr_bytes_to_find: &[u8],
    key_bytes_to_find: &[u8],
) -> anyhow::Result<Option<Bytes>> {
    do_search_2(
        c,
        block_number,
        key_prefix_len,
        addr_bytes_to_find,
        key_bytes_to_find,
        0,
    )
    .await
}

pub async fn do_search_2<C: CursorDupSort>(
    c: &mut C,
    block_number: u64,
    key_prefix_len: usize,
    addr_bytes_to_find: &[u8],
    key_bytes_to_find: &[u8],
    incarnation: u64,
) -> anyhow::Result<Option<Bytes>> {
    if incarnation == 0 {
        let mut seek = vec![0; common::BLOCK_NUMBER_LENGTH + key_prefix_len];
        seek[..].as_mut().write(&block_number.to_be_bytes());
        seek[8..].as_mut().write(addr_bytes_to_find).unwrap();
        let (mut k, mut v) = c.seek(&*seek).await?;
        loop {
            if k.is_empty() {
                break;
            }

            let (_, k1, v1) = from_storage_db_format(key_prefix_len)(k, v);
            if !k1.starts_with(addr_bytes_to_find) {
                break;
            }

            let st_hash = &k1[key_prefix_len + common::INCARNATION_LENGTH..];
            if st_hash == key_bytes_to_find {
                return Ok(Some(v1));
            }

            (k, v) = c.next().await?
        }

        return Ok(None);
    }

    let mut seek =
        vec![0; common::BLOCK_NUMBER_LENGTH + key_prefix_len + common::INCARNATION_LENGTH];
    seek[..common::BLOCK_NUMBER_LENGTH].copy_from_slice(&block_number.to_be_bytes());
    seek[common::BLOCK_NUMBER_LENGTH..]
        .as_mut()
        .write(addr_bytes_to_find)
        .unwrap();
    seek[common::BLOCK_NUMBER_LENGTH + key_prefix_len..]
        .copy_from_slice(&incarnation.to_be_bytes());

    let (k, v) = c.seek_both_range(&seek, key_bytes_to_find).await?;
    if k.is_empty() {
        return Ok(None);
    }

    if !v.starts_with(key_bytes_to_find) {
        return Ok(None);
    }

    let (_, _, v) = from_storage_db_format(key_prefix_len)(k, v);

    Ok(Some(v))
}

pub fn encode_storage<Key: Eq + Ord + AsRef<[u8]>>(
    block_n: u64,
    s: &ChangeSet<Key>,
    key_prefix_len: usize,
) -> impl Iterator<Item = (Bytes, Bytes)> + '_ {
    s.iter().map(move |cs| {
        let cs_key = cs.key.as_ref();

        let key_part = key_prefix_len + common::INCARNATION_LENGTH;

        let mut new_k = vec![0; common::BLOCK_NUMBER_LENGTH + key_part];
        new_k[..common::BLOCK_NUMBER_LENGTH].copy_from_slice(&encode_block_number(block_n));
        new_k[common::BLOCK_NUMBER_LENGTH..].copy_from_slice(&cs_key[..key_part]);

        let mut new_v = BytesMut::with_capacity(common::HASH_LENGTH + cs.value.len());
        new_v.put_slice(&cs_key[key_part..]);
        new_v.put_slice(&cs.value[..]);

        (new_k.into(), new_v.freeze())
    })
}