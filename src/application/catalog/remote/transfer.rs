use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::{anyhow, ensure, Result};
use futures_util::{StreamExt, TryStreamExt};

use super::*;

struct Input {
    url: String,
    filename: String,
    limit: u64,
    size: Option<u64>,
    auxiliary: bool,
}

#[derive(Default)]
struct Progress {
    received: u64,
    total: Option<u64>,
    started: bool,
}

pub(super) async fn download(
    transfer: &crate::plugin::source::DiscoverTransfer,
    staged: &StagedArtifact,
    progress: &mut (impl FnMut(Option<RemoteDownloadTransfer>) + Send),
) -> Result<(PathBuf, BTreeMap<String, String>)> {
    crate::remote_asset::prepare::validate_resources(&transfer.resources)?;
    let main_url = crate::remote_asset::prepare::validate_url(&transfer.url)?.to_string();
    let mut inputs = vec![Input {
        url: main_url.clone(),
        filename: "input-0".into(),
        limit: DOWNLOAD_LIMIT,
        size: None,
        auxiliary: false,
    }];
    let mut urls = HashMap::from([(main_url, 0_usize)]);
    let mut resources = BTreeMap::new();
    for resource in &transfer.resources {
        let url = crate::remote_asset::prepare::validate_url(&resource.url)?.to_string();
        let index = if let Some(index) = urls.get(&url) {
            *index
        } else {
            let index = inputs.len();
            urls.insert(url.clone(), index);
            inputs.push(Input {
                url,
                filename: format!("input-{index}"),
                limit: crate::remote_asset::prepare::RESOURCE_LIMIT,
                size: None,
                auxiliary: true,
            });
            index
        };
        let input = &mut inputs[index];
        if let (Some(previous), Some(size)) = (input.size, resource.size) {
            ensure!(
                previous == size,
                "duplicate resource URL has inconsistent sizes"
            );
        }
        input.size = input.size.or(resource.size);
        input.auxiliary = true;
        input.limit = crate::remote_asset::prepare::RESOURCE_LIMIT;
        resources.insert(resource.id.clone(), input.filename.clone());
    }
    let states = (0..inputs.len())
        .map(|_| Progress::default())
        .collect::<Vec<_>>();
    let aggregate = Mutex::new((states, progress));
    let input_budget = AtomicU64::new(0);
    let input_root = staged.input_root();
    let primary = input_root.join(&inputs[0].filename);
    let futures = inputs.into_iter().enumerate().map(|(index, input)| {
        let aggregate = &aggregate;
        let input_budget = &input_budget;
        let path = input_root.join(input.filename);
        async move {
            download_remote_file_checked(
                &input.url,
                &path,
                input.limit,
                input.size,
                input.auxiliary,
                input.auxiliary.then_some(input_budget),
                |value| {
                    let mut aggregate = aggregate
                        .lock()
                        .map_err(|_| anyhow!("download progress poisoned"))?;
                    let (states, publish) = &mut *aggregate;
                    states[index] = Progress {
                        received: value.received_bytes,
                        total: value.total_bytes,
                        started: true,
                    };
                    let received_bytes = states.iter().map(|state| state.received).sum();
                    let total_bytes = states
                        .iter()
                        .map(|state| state.started.then_some(state.total).flatten())
                        .sum();
                    publish(Some(RemoteDownloadTransfer {
                        received_bytes,
                        total_bytes,
                    }));
                    Ok(())
                },
            )
            .await
        }
    });
    futures_util::stream::iter(futures)
        .buffer_unordered(2)
        .try_collect::<Vec<_>>()
        .await?;
    let (_, progress) = aggregate
        .into_inner()
        .map_err(|_| anyhow!("download progress poisoned"))?;
    progress(None);
    Ok((primary, resources))
}
