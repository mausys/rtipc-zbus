use std::num::NonZeroUsize;
use std::os::fd::BorrowedFd;
use zbus::{Connection, fdo::Error as ZBusError, proxy};

use rtipc::{
    ChannelConfig, ChannelVector, ConsumeResult, Consumer, Producer, QueueConfig, VectorConfig,
    VectorResource,
};

use rtipc_zbus::{
    ChannelConfigBus, CommandId, MsgCommand, MsgEvent, MsgResponse, rtipc_into_zbus_config,
};

pub fn to_owned_fd(fd: BorrowedFd<'_>) -> Result<zvariant::OwnedFd, ZBusError> {
    fd.try_clone_to_owned()
        .map_err(|e| ZBusError::Failed(String::from("try_clone_to_owned failed")))
        .map(|fd| zvariant::OwnedFd::from(fd))
}

#[proxy(
    interface = "org.rtipc.server",
    default_service = "org.rtipc.server",
    default_path = "/org/rtipc/server"
)]
trait Server {
    async fn connect(
        &self,
        shmfd_bus: zvariant::OwnedFd,
        consumers_zbus: Vec<ChannelConfigBus>,
        consumers_eventfds: Vec<zvariant::OwnedFd>,
        producers_zbus: Vec<ChannelConfigBus>,
        producers_eventfds: Vec<zvariant::OwnedFd>,
        info: Vec<u8>,
    ) -> Result<String, ZBusError>;
}

// Although we use `tokio` here, you can use any async runtime of choice.
#[tokio::main]
async fn main() -> Result<(), ZBusError> {
    let commands: [MsgCommand; 6] = [
        MsgCommand {
            id: CommandId::Hello as u32,
            args: [1, 2, 0],
        },
        MsgCommand {
            id: CommandId::SendEvent as u32,
            args: [11, 20, 0],
        },
        MsgCommand {
            id: CommandId::SendEvent as u32,
            args: [12, 20, 1],
        },
        MsgCommand {
            id: CommandId::Div as u32,
            args: [100, 7, 0],
        },
        MsgCommand {
            id: CommandId::Div as u32,
            args: [100, 0, 0],
        },
        MsgCommand {
            id: CommandId::Stop as u32,
            args: [0, 0, 0],
        },
    ];

    let c2s_channels: [ChannelConfig; 1] = [ChannelConfig {
        queue: QueueConfig {
            additional_messages: 0,
            message_size: unsafe { NonZeroUsize::new_unchecked(size_of::<MsgCommand>()) },
            info: b"rpc command".to_vec(),
        },
        eventfd: true,
    }];

    let s2c_channels: [ChannelConfig; 2] = [
        ChannelConfig {
            queue: QueueConfig {
                additional_messages: 0,
                message_size: unsafe { NonZeroUsize::new_unchecked(size_of::<MsgResponse>()) },
                info: b"rpc response".to_vec(),
            },
            eventfd: false,
        },
        ChannelConfig {
            queue: QueueConfig {
                additional_messages: 10,
                message_size: unsafe { NonZeroUsize::new_unchecked(size_of::<MsgEvent>()) },
                info: b"rpc event".to_vec(),
            },
            eventfd: true,
        },
    ];

    let vconfig = VectorConfig {
        producers: c2s_channels.to_vec(),
        consumers: s2c_channels.to_vec(),
        info: b"rpc example".to_vec(),
    };

    let resource = VectorResource::allocate(&vconfig)
        .map_err(|_| ZBusError::InvalidArgs(String::from("VectorResource failed")))?;

    let consumers_zbus = rtipc_into_zbus_config(&vconfig.consumers);
    let producers_zbus = rtipc_into_zbus_config(&vconfig.producers);

    let shmfd = to_owned_fd(resource.shmfd())?;

    let consumer_eventfds: Vec<zvariant::OwnedFd> = resource
        .collect_consumer_eventfds()
        .into_iter()
        .map(to_owned_fd)
        .collect::<Result<Vec<zvariant::OwnedFd>, ZBusError>>()?;

    let producer_eventfds: Vec<zvariant::OwnedFd> = resource
        .collect_producer_eventfds()
        .into_iter()
        .map(to_owned_fd)
        .collect::<Result<Vec<zvariant::OwnedFd>, ZBusError>>()?;

    let connection = Connection::session().await?;

    let proxy = ServerProxy::new(&connection).await?;
    let reply = proxy
        .connect(
            shmfd,
            consumers_zbus,
            consumer_eventfds,
            producers_zbus,
            producer_eventfds,
            vconfig.info,
        )
        .await?;

    let vec = ChannelVector::new(resource)
        .map_err(|_| ZBusError::InvalidArgs(String::from("ChannelVector filed")))?;

    Ok(())
}

fn handle_events(mut consumer: Consumer<MsgEvent>) -> Result<(), ZBusError> {
    match consumer.pop() {
        ConsumeResult::QueueError => panic!(),
        ConsumeResult::NoMessage => {
            return Err(ZBusError::Failed(String::from("ConsumeResult::NoMessage")));
        }
        ConsumeResult::NoNewMessage => {
            return Err(ZBusError::Failed(String::from("ConsumeResult::NoMessage")));
        }
        ConsumeResult::Success => {
            println!(
                "client received event: {}",
                consumer.current_message().unwrap()
            )
        }
        ConsumeResult::SuccessMessagesDiscarded => {
            println!(
                "client received event: {}",
                consumer.current_message().unwrap()
            )
        }
    };
    println!("handle_events returns");
    Ok(())
}

struct App {
    command: Producer<MsgCommand>,
    response: Consumer<MsgResponse>,
}

impl App {
    pub fn new(mut vec: ChannelVector) -> Self {
        let command = vec.take_producer(0).unwrap();
        let response = vec.take_consumer(0).unwrap();

        Self { command, response }
    }

    pub fn run(&mut self, cmds: &[MsgCommand]) {
        for cmd in cmds {
            self.command.current_message().clone_from(cmd);
            self.command.force_push();

            loop {
                match self.response.pop() {
                    ConsumeResult::QueueError => panic!(),
                    ConsumeResult::NoMessage => {
                        continue;
                    }
                    ConsumeResult::NoNewMessage => {
                        continue;
                    }
                    ConsumeResult::Success => {}
                    ConsumeResult::SuccessMessagesDiscarded => {}
                };

                println!(
                    "client received response: {}",
                    self.response.current_message().unwrap()
                );
                break;
            }
        }
    }
}
