use crate::{
    interface_config::{InterfaceConfig, InterfaceInfo, ServerInfo},
    Association, Cidr, CidrContents, CidrTree, Error, Peer, PeerContents,
    PERSISTENT_KEEPALIVE_INTERVAL_SECS,
};
use colored::*;
use dialoguer::{theme::ColorfulTheme, Confirm, Input, Select};
use ipnetwork::IpNetwork;
use lazy_static::lazy_static;
use regex::Regex;
use std::net::{IpAddr, SocketAddr};
use wgctrl::KeyPair;

lazy_static! {
    static ref THEME: ColorfulTheme = ColorfulTheme::default();

    /// Regex to match the requirements of hostname(7), needed to have peers also be reachable hostnames.
    /// Note that the full length also must be maximum 63 characters, which this regex does not check.
    static ref PEER_NAME_REGEX: Regex = Regex::new(r"^([a-z0-9]-?)*[a-z0-9]$").unwrap();
}

pub fn is_valid_hostname(name: &str) -> bool {
    name.len() < 64 && PEER_NAME_REGEX.is_match(name)
}

pub fn hostname_validator(name: &String) -> Result<(), &'static str> {
    if is_valid_hostname(name) {
        Ok(())
    } else {
        Err("not a valid hostname")
    }
}

/// Bring up a prompt to create a new CIDR. Returns the peer request.
pub fn add_cidr(cidrs: &[Cidr]) -> Result<Option<CidrContents>, Error> {
    let parent_cidr = choose_cidr(cidrs, "Parent CIDR")?;
    let name: String = Input::with_theme(&*THEME).with_prompt("Name").interact()?;
    let cidr: IpNetwork = Input::with_theme(&*THEME).with_prompt("CIDR").interact()?;

    let cidr_request = CidrContents {
        name,
        cidr,
        parent: Some(parent_cidr.id),
    };

    Ok(
        if Confirm::with_theme(&*THEME)
            .with_prompt(&format!("Create CIDR \"{}\"?", cidr_request.name))
            .default(false)
            .interact()?
        {
            Some(cidr_request)
        } else {
            None
        },
    )
}

pub fn choose_cidr<'a>(cidrs: &'a [Cidr], text: &'static str) -> Result<&'a Cidr, Error> {
    let cidr_names: Vec<_> = cidrs
        .iter()
        .map(|cidr| format!("{} ({})", &cidr.name, &cidr.cidr))
        .collect();
    let cidr_index = Select::with_theme(&*THEME)
        .with_prompt(text)
        .items(&cidr_names)
        .interact()?;
    Ok(&cidrs[cidr_index])
}

pub fn choose_association<'a>(
    associations: &'a [Association],
    cidrs: &'a [Cidr],
) -> Result<&'a Association, Error> {
    let names: Vec<_> = associations
        .iter()
        .map(|association| {
            format!(
                "{}: {} <=> {}",
                association.id,
                &cidrs
                    .iter()
                    .find(|c| c.id == association.cidr_id_1)
                    .unwrap()
                    .name,
                &cidrs
                    .iter()
                    .find(|c| c.id == association.cidr_id_2)
                    .unwrap()
                    .name
            )
        })
        .collect();
    let index = Select::with_theme(&*THEME)
        .with_prompt("Association")
        .items(&names)
        .interact()?;

    Ok(&associations[index])
}

pub fn add_association(cidrs: &[Cidr]) -> Result<Option<(&Cidr, &Cidr)>, Error> {
    let cidr1 = choose_cidr(&cidrs[..], "First CIDR")?;
    let cidr2 = choose_cidr(&cidrs[..], "Second CIDR")?;

    Ok(
        if Confirm::with_theme(&*THEME)
            .with_prompt(&format!(
                "Add association: {} <=> {}?",
                cidr1.name.yellow().bold(),
                cidr2.name.yellow().bold()
            ))
            .default(false)
            .interact()?
        {
            Some((cidr1, cidr2))
        } else {
            None
        },
    )
}

pub fn delete_association<'a>(
    associations: &'a [Association],
    cidrs: &'a [Cidr],
) -> Result<Option<&'a Association>, Error> {
    let association = choose_association(associations, cidrs)?;

    Ok(
        if Confirm::with_theme(&*THEME)
            .with_prompt(&format!("Delete association #{}?", association.id))
            .default(false)
            .interact()?
        {
            Some(association)
        } else {
            None
        },
    )
}

/// Bring up a prompt to create a new peer. Returns the peer request.
pub fn add_peer(
    peers: &[Peer],
    cidr_tree: &CidrTree,
) -> Result<Option<(PeerContents, KeyPair)>, Error> {
    let leaves = cidr_tree.leaves();

    let cidr = choose_cidr(&leaves[..], "Eligible CIDRs for peer")?;

    let mut available_ip = None;
    let candidate_ips = cidr.iter().filter(|ip| cidr.is_assignable(*ip));
    for ip in candidate_ips {
        if peers.iter().find(|peer| peer.ip == ip).is_none() {
            available_ip = Some(ip);
            break;
        }
    }

    let available_ip = available_ip.expect("No IPs in this CIDR are avavilable");

    let ip = Input::with_theme(&*THEME)
        .with_prompt("IP")
        .default(available_ip)
        .interact()?;

    let name: String = Input::with_theme(&*THEME)
        .with_prompt("Name")
        .validate_with(hostname_validator)
        .interact()?;

    let is_admin = Confirm::with_theme(&*THEME)
        .with_prompt(&format!("Make {} an admin?", name))
        .default(false)
        .interact()?;
    let default_keypair = KeyPair::generate();
    let peer_request = PeerContents {
        name,
        ip,
        cidr_id: cidr.id,
        public_key: default_keypair.public.to_base64(),
        endpoint: None,
        is_admin,
        is_disabled: false,
        is_redeemed: false,
        persistent_keepalive_interval: Some(PERSISTENT_KEEPALIVE_INTERVAL_SECS),
    };

    Ok(
        if Confirm::with_theme(&*THEME)
            .with_prompt(&format!("Create peer {}?", peer_request.name.yellow()))
            .default(false)
            .interact()?
        {
            Some((peer_request, default_keypair))
        } else {
            None
        },
    )
}

/// Presents a selection and confirmation of eligible peers for either disabling or enabling,
/// and returns back the ID of the selected peer.
pub fn enable_or_disable_peer(peers: &[Peer], enable: bool) -> Result<Option<Peer>, Error> {
    let enabled_peers: Vec<_> = peers
        .iter()
        .filter(|peer| enable && peer.is_disabled || !enable && !peer.is_disabled)
        .collect();

    let peer_selection: Vec<_> = enabled_peers
        .iter()
        .map(|peer| format!("{} ({})", &peer.name, &peer.ip))
        .collect();
    let index = Select::with_theme(&*THEME)
        .with_prompt(&format!(
            "Peer to {}able",
            if enable { "en" } else { "dis" }
        ))
        .items(&peer_selection)
        .interact()?;
    let peer = enabled_peers[index];

    Ok(
        if Confirm::with_theme(&*THEME)
            .with_prompt(&format!(
                "{}able peer {}?",
                if enable { "En" } else { "Dis" },
                peer.name.yellow()
            ))
            .default(false)
            .interact()?
        {
            Some(peer.clone())
        } else {
            None
        },
    )
}

/// Confirm and write a innernet invitation file after a peer has been created.
pub fn save_peer_invitation(
    network_name: &str,
    peer: &Peer,
    server_peer: &Peer,
    root_cidr: &Cidr,
    keypair: KeyPair,
    server_api_addr: &SocketAddr,
) -> Result<(), Error> {
    let peer_invitation = InterfaceConfig {
        interface: InterfaceInfo {
            network_name: network_name.to_string(),
            private_key: keypair.private.to_base64(),
            address: IpNetwork::new(peer.ip, root_cidr.prefix())?,
            listen_port: None,
        },
        server: ServerInfo {
            external_endpoint: server_peer
                .endpoint
                .expect("The innernet server should have a WireGuard endpoint"),
            internal_endpoint: *server_api_addr,
            public_key: server_peer.public_key.clone(),
        },
    };

    let invitation_save_path = Input::with_theme(&*THEME)
        .with_prompt("Save peer invitation file as")
        .default(format!("{}.toml", peer.name))
        .interact()?;

    peer_invitation.write_to_path(&invitation_save_path, true, None)?;

    println!(
        "\nPeer \"{}\" added\n\
         Peer invitation file written to {}\n\
         Please send it to them securely (eg. via magic-wormhole) \
         to bootstrap them onto the network.",
        peer.name.bold(),
        invitation_save_path.bold()
    );

    Ok(())
}

pub fn set_listen_port(
    interface: &InterfaceInfo,
    unset: bool,
) -> Result<Option<Option<u16>>, Error> {
    let listen_port = (!unset)
        .then(|| {
            Input::with_theme(&*THEME)
                .with_prompt("Listen port")
                .default(interface.listen_port.unwrap_or(51820))
                .interact()
        })
        .transpose()?;

    let mut confirmation = Confirm::with_theme(&*THEME);
    confirmation
        .with_prompt(
            &(if let Some(port) = &listen_port {
                format!("Set listen port to {}?", port)
            } else {
                "Unset and randomize listen port?".to_string()
            }),
        )
        .default(false);

    if listen_port == interface.listen_port {
        println!("No change necessary - interface already has this setting.");
        Ok(None)
    } else if confirmation.interact()? {
        Ok(Some(listen_port))
    } else {
        Ok(None)
    }
}

pub fn ask_endpoint() -> Result<SocketAddr, Error> {
    println!("getting external IP address.");

    let external_ip: Option<IpAddr> = ureq::get("http://4.icanhazip.com")
        .call()
        .ok()
        .map(|res| res.into_string().ok())
        .flatten()
        .map(|body| body.trim().to_string())
        .and_then(|body| body.parse().ok());

    let mut endpoint_builder = Input::with_theme(&*THEME);
    if let Some(ip) = external_ip {
        endpoint_builder.default(SocketAddr::new(ip, 51820));
    } else {
        println!("failed to get external IP.");
    }
    endpoint_builder
        .with_prompt("External endpoint")
        .interact()
        .map_err(|e| Error::from(e))
}

pub fn override_endpoint(unset: bool) -> Result<Option<Option<SocketAddr>>, Error> {
    let endpoint = if !unset { Some(ask_endpoint()?) } else { None };

    Ok(
        if Confirm::with_theme(&*THEME)
            .with_prompt(
                &(if let Some(endpoint) = &endpoint {
                    format!("Set external endpoint to {}?", endpoint)
                } else {
                    "Unset external endpoint to enable automatic endpoint discovery?".to_string()
                }),
            )
            .default(false)
            .interact()?
        {
            Some(endpoint)
        } else {
            None
        },
    )
}