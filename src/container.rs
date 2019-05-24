use log::{debug, error, info, warn};
use rand::{distributions::Alphanumeric, thread_rng, Rng};
use shiplift::{rep::ContainerDetails, ContainerOptions, Docker, PullOptions, RmContainerOptions};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::prelude::{future, Future, Stream};

pub type Client = Arc<Docker>;

pub enum Protocol {
    Tcp,
    Udp,
}

impl AsRef<str> for Protocol {
    fn as_ref(&self) -> &str {
        match self {
            Protocol::Tcp => "tcp",
            Protocol::Udp => "udp",
        }
    }
}

pub struct Port {
    pub source: u32,
    pub host: u32,
    pub protocol: Protocol,
}

pub struct Container {
    pub(crate) details: ContainerDetails,

    client: Arc<Docker>,
}

impl Container {
    pub fn new(
        client: &Client,
        image_name: impl Into<String>,
    ) -> impl Future<Item = Self, Error = shiplift::Error> {
        ContainerBuilder::new(client, image_name).build()
    }

    pub fn builder(client: &Client, image_name: impl Into<String>) -> ContainerBuilder {
        ContainerBuilder::new(client, image_name)
    }

    pub fn id(&self) -> &str {
        &self.details.id
    }

    pub fn ports(&self) -> &Option<HashMap<String, Option<Vec<HashMap<String, String>>>>> {
        &self.details.network_settings.ports
    }

    pub fn delete(self) -> impl Future<Item = (), Error = shiplift::Error> {
        self.client
            .containers()
            .get(&self.id())
            .remove(RmContainerOptions::builder().force(true).build())
    }
}

pub struct ContainerBuilder {
    image_name: String,
    image_tag: String,
    name: Option<String>,
    ports: Vec<Port>,

    client: Arc<Docker>,

    pull_on_build: bool,
    slug_length: usize,
}

impl ContainerBuilder {
    pub fn new(client: &Client, image_name: impl Into<String>) -> Self {
        ContainerBuilder {
            image_name: image_name.into(),
            image_tag: String::from("latest"),
            name: None,
            ports: Vec::new(),

            client: Arc::clone(client),

            pull_on_build: false,
            slug_length: 0,
        }
    }

    fn image(&self) -> String {
        format!("{}:{}", self.image_name, self.image_tag)
    }

    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    pub fn pull_image(&self) -> impl Future<Item = (), Error = shiplift::Error> {
        pull_image(Arc::clone(&self.client), self.image()).map(|_| ())
    }

    pub fn slug_length(mut self, length: usize) -> Self {
        self.slug_length = length;
        self
    }

    fn slugged_name(&self) -> Option<String> {
        let base_name = self.name.clone()?;

        if self.slug_length > 0 {
            let slug: String = thread_rng()
                .sample_iter(&Alphanumeric)
                .take(self.slug_length)
                .collect();

            Some(base_name + "_" + &slug)
        } else {
            Some(base_name)
        }
    }

    pub fn expose(mut self, port: Port) -> Self {
        self.ports.push(port);
        self
    }

    pub fn build(self) -> impl Future<Item = Container, Error = shiplift::Error> {
        let image = self.image();
        let name = self.slugged_name();
        let ports = self.ports;

        pull_image_if(self.client, image, self.pull_on_build)
            .and_then(|(client, image)| create_container(client, image, name, ports))
            .and_then(|(client, id)| run_container(client, id))
            .and_then(|(client, id)| inspect_container(client, id))
            .map(|(client, details)| Container { details, client })
    }
}

fn pull_image_if(
    client: Client,
    image: String,
    pull: bool,
) -> impl Future<Item = (Client, String), Error = shiplift::Error> {
    match pull {
        true => future::Either::A(pull_image(client, image)),
        false => future::Either::B(future::ok((client, image))),
    }
}

fn pull_image(
    client: Client,
    image: String,
) -> impl Future<Item = (Client, String), Error = shiplift::Error> {
    info!("pulling image: {}", &image);

    client
        .images()
        .pull(&PullOptions::builder().image(&image).build())
        .for_each(|output| {
            debug!("{:?}", output);
            Ok(())
        })
        .map(move |_| {
            info!("pulled image: {}", &image);
            (client, image)
        })
}

fn create_container<S: AsRef<str>>(
    client: Client,
    image: String,
    container_name: Option<S>,
    ports: impl IntoIterator<Item = Port>,
) -> impl Future<Item = (Client, String), Error = shiplift::Error> {
    let mut container_options = ContainerOptions::builder(image.as_ref());

    if let Some(name) = container_name.as_ref() {
        container_options.name(name.as_ref());
    }

    for port in ports {
        container_options.expose(port.source, port.protocol.as_ref(), port.host);
    }

    client
        .containers()
        .create(&container_options.build())
        .map(|info| (client, info.id))
}

fn run_container(
    client: Client,
    id: String,
) -> impl Future<Item = (Client, String), Error = shiplift::Error> {
    client.containers().get(&id).start().map(|_| (client, id))
}

fn inspect_container(
    client: Client,
    id: String,
) -> impl Future<Item = (Client, ContainerDetails), Error = shiplift::Error> {
    client
        .containers()
        .get(&id)
        .inspect()
        .map(|details| (client, details))
}