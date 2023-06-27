//! Common bits shared between caldav and carddav clients.

use crate::{
    dav::WebDavClient,
    dns::{find_context_path_via_txt_records, resolve_srv_record, DiscoverableService},
    BootstrapError,
};
use domain::base::Dname;

use hyper::Uri;

/// A big chunk of the bootstrap logic that's shared between both types.
///
/// Mutates the `base_url` for the client to the discovered one.
pub(crate) async fn common_bootstrap(
    client: &mut WebDavClient,
    port: u16,
    service: DiscoverableService,
) -> Result<(), BootstrapError> {
    let domain = client
        .base_url
        .host()
        .ok_or(BootstrapError::InvalidUrl("a host is required"))?;

    let dname = Dname::bytes_from_str(domain)
        .map_err(|_| BootstrapError::InvalidUrl("invalid domain name"))?;
    let host_candidates = {
        let candidates = resolve_srv_record(service, &dname, port)
            .await
            .map_err(BootstrapError::DnsError)?;

        // If there are no SRV records, try the domain/port in the provided URI.
        if candidates.is_empty() {
            vec![(domain.to_string(), port)]
        } else {
            candidates
        }
    };

    if let Some(path) = find_context_path_via_txt_records(service, &dname).await? {
        let candidate = &host_candidates[0];

        // TODO: check `DAV:` capabilities here.
        client.base_url = Uri::builder()
            .scheme(service.scheme())
            .authority(format!("{}:{}", candidate.0, candidate.1))
            .path_and_query(path)
            .build()
            .map_err(BootstrapError::UnusableSrv)?;
    } else {
        for candidate in host_candidates {
            if let Ok(Some(url)) = client
                .find_context_path(service, &candidate.0, candidate.1)
                .await
            {
                client.base_url = url;
                break;
            }
        }
    }

    client.principal = client.find_current_user_principal().await?;

    Ok(())
}
