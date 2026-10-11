//! Pictures in a bucket (`s3.rs`), against a real S3-compatible store:
//! with the `s3` feature, and `CARNET_TEST_S3` naming a bucket
//! (`http://127.0.0.1:9000/carnet-test`, made if it isn't there) whose
//! keys are in `CARNET_S3_ACCESS_KEY` and `CARNET_S3_SECRET_KEY`. Each
//! test keeps to a folder of its own, and empties it after. Without them,
//! the tests say so and pass.
#![cfg(feature = "s3")]

mod common;

use carnet_server::blobs::{Blobs, Job, MAX, Worker};
use carnet_server::s3::S3Blobs;
use carnet_server::Out;
use carnet_sync::wire::{self, Carnet, Frame};
use common::{bucket, empty};

fn said(w: &mut Worker<S3Blobs>, job: Job) -> Vec<Carnet> {
    let mut outs = Vec::new();
    w.run(job, &mut |o| outs.push(o));
    outs.iter()
        .filter_map(|o| match o {
            Out::Send(_, b) => match wire::decode(b) {
                Ok(Frame::Carnet(c)) => Some(c),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

fn agent() -> ureq::Agent {
    ureq::Agent::new_with_config(ureq::Agent::config_builder().http_status_as_error(false).build())
}

/// Offered: a link to put it straight into the bucket; put there, it's
/// kept, and its fetch link gets it back, for the link's time in a
/// browser's cache. A link one way doesn't go the other, or changed.
#[test]
fn a_picture_goes_straight_to_the_bucket_and_back() {
    let Some((s3, config)) = bucket("roundtrip") else { return };
    let mut w = Worker::new(s3.clone());
    let acct = b"acct".to_vec();
    let offer = Job::Offer { conn: 1, account: acct.clone(), ids: vec![[1; 32]], room: None, writable: true };
    let [Carnet::BlobLink { put: true, url: put, .. }] = &said(&mut w, offer.clone())[..] else { panic!("a put link") };
    assert!(put.starts_with(config.url.split("/roundtrip").next().unwrap()), "the bucket's own: {put}");
    let sealed: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    let mut changed = put.clone();
    changed.pop();
    changed.push(if put.ends_with('0') { '1' } else { '0' });
    assert_eq!(agent().put(&changed).send(&sealed[..]).unwrap().status().as_u16(), 403, "a changed link puts nothing");
    let r = agent().put(put).header("Content-Type", "application/octet-stream").send(&sealed[..]).unwrap();
    assert!(r.status().is_success(), "{}", r.status());
    assert_eq!(said(&mut w, offer), [Carnet::BlobStored { id: [1; 32] }]);
    assert_eq!(w.blobs.usage(&acct), sealed.len() as u64);

    let fetch = Job::Fetch { conn: 1, account: acct.clone(), ids: vec![[1; 32], [2; 32]] };
    let fetched = said(&mut w, fetch);
    let [Carnet::BlobLink { put: false, url: get, .. }, Carnet::BlobNone { id: missing }] = &fetched[..] else { panic!("{fetched:?}") };
    assert_eq!(*missing, [2; 32]);
    let mut r = agent().get(get).call().unwrap();
    assert_eq!(r.status().as_u16(), 200);
    let cache = r.headers().get("cache-control").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
    assert!(cache.starts_with("private, max-age="), "{cache}");
    assert_eq!(r.body_mut().with_config().limit(MAX).read_to_vec().unwrap(), sealed);
    assert_eq!(agent().put(get).send(&b"other"[..]).unwrap().status().as_u16(), 403, "a get link doesn't put");

    // A put past what a picture may be: never said kept, and gone.
    let big = vec![7u8; MAX as usize + 1];
    s3.put(&s3.key(&acct, &[3; 32]), &big).unwrap();
    assert_eq!(w.blobs.size(&acct, &[3; 32]), None);
    assert!(s3.head(&s3.key(&acct, &[3; 32])).unwrap().is_none(), "deleted");

    // An account removed: its pictures go.
    w.run(Job::Gone { account: acct.clone() }, &mut |_| {});
    assert_eq!(w.blobs.size(&acct, &[1; 32]), None);
    assert_eq!(w.blobs.usage(&acct), 0);
    empty(&s3, &config);
}

/// Marked, kept a while, then gone; unmarked, it stays; as the files do.
/// And a bucket says once a server has run on it.
#[test]
fn marks_and_sweeps_as_files_do() {
    let Some((mut s3, config)) = bucket("marks") else { return };
    let acct = b"acct";
    s3.put(&s3.key(acct, &[1; 32]), b"sealed").unwrap();
    s3.put(&s3.key(acct, &[2; 32]), b"other").unwrap();
    s3.mark(acct, &[1; 32], 1000);
    s3.mark(acct, &[2; 32], 1000);
    s3.mark(acct, &[1; 32], 5000);
    s3.mark(acct, &[3; 32], 1000);
    assert_eq!(s3.marked(acct, &[3; 32]).unwrap(), None, "not kept: nothing to mark");
    assert_eq!(s3.marked(acct, &[1; 32]).unwrap(), Some(1000), "the first mark counts");
    s3.unmark(acct, &[2; 32]);
    assert_eq!(s3.sweep(1000), 0, "marked at 1000: not before it");
    assert_eq!(s3.sweep(1001), 1);
    assert_eq!((s3.size(acct, &[1; 32]), s3.size(acct, &[2; 32])), (None, Some(5)));
    assert_eq!(s3.marked(acct, &[1; 32]).unwrap(), None);
    assert_eq!(s3.usage(acct), 5, "marks aren't pictures");

    assert!(!s3.served().unwrap());
    s3.serving().unwrap();
    assert!(s3.served().unwrap());
    empty(&s3, &config);
}
