use std::future::pending;
use std::num::NonZeroUsize;
use std::os::fd::BorrowedFd;
use tokio::time::{Duration, sleep};

use zbus::{Connection, fdo::Error as ZBusError, proxy};

use rtipc::{
    ChannelConfig, ChannelVector, ConsumeResult, Consumer, Producer, QueueConfig, VectorConfig,
    VectorResource,
};

use rtipc_zbus::{
    AsyncEventFd, ChannelConfigBus, CommandId, MsgCommand, MsgEvent, MsgResponse,
    rtipc_into_zbus_config,
};

pub fn to_owned_fd(fd: BorrowedFd<'_>) -> Result<zvariant::OwnedFd, ZBusError> {
    fd.try_clone_to_owned()
        .map_err(|_| ZBusError::Failed(String::from("try_clone_to_owned failed")))
        .map(zvariant::OwnedFd::from)
}
// a producer on the client side is a consumer on the server side
// and vice versa
#[proxy(
    interface = "org.rtipc.server",
    default_service = "org.rtipc.server",
    default_path = "/org/rtipc/server"
)]
trait Server {
    async fn connect(
        &self,
        shmfd_bus: zvariant::OwnedFd,
        producers_zbus: Vec<ChannelConfigBus>,
        producers_eventfds: Vec<zvariant::OwnedFd>,
        consumers_zbus: Vec<ChannelConfigBus>,
        consumers_eventfds: Vec<zvariant::OwnedFd>,
        info: Vec<u8>,
    ) -> Result<(), ZBusError>;
}

async fn listen_events(mut event: Consumer<MsgEvent>) -> Result<(), ZBusError> {
    for _ in 0..100 {
        sleep(Duration::from_millis(10)).await;
        match event.pop() {
            ConsumeResult::QueueError => panic!(),
            ConsumeResult::NoMessage => {
                continue;
            }
            ConsumeResult::NoNewMessage => {
                continue;
            }
            ConsumeResult::Success => {
                println!(
                    "client received event: {}",
                    event.current_message().unwrap()
                )
            }
            ConsumeResult::SuccessMessagesDiscarded => {
                println!(
                    "client received event: {}",
                    event.current_message().unwrap()
                )
            }
        };
    }
    println!("handle_events returns");
    Ok(())
}

async fn exec_commands(
    cmds: &[MsgCommand],
    mut command: Producer<MsgCommand>,
    mut response: Consumer<MsgResponse>,
) {
    let fd = response.take_eventfd().unwrap();
    let async_fd = AsyncEventFd::new(fd).unwrap();

    for cmd in cmds {
        command.current_message().clone_from(cmd);
        command.force_push();

        async_fd
            .await_event()
            .await
            .inspect_err(|e| println!("await_event error {e}"))
            .unwrap();

        match response.pop() {
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
            response.current_message().unwrap()
        );
    }
}

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
            eventfd: true,
        },
        ChannelConfig {
            queue: QueueConfig {
                additional_messages: 10,
                message_size: unsafe { NonZeroUsize::new_unchecked(size_of::<MsgEvent>()) },
                info: b"rpc event".to_vec(),
            },
            eventfd: false,
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
    proxy
        .connect(
            shmfd,
            producers_zbus,
            producer_eventfds,
            consumers_zbus,
            consumer_eventfds,
            vconfig.info,
        )
        .await?;

    let mut vec = ChannelVector::new(resource)
        .map_err(|_| ZBusError::InvalidArgs(String::from("ChannelVector filed")))?;

    let command = vec.take_producer(0).unwrap();
    let response = vec.take_consumer(0).unwrap();
    let event = vec.take_consumer(1).unwrap();

    tokio::spawn(async move {
        listen_events(event).await.unwrap();
    });

    tokio::spawn(async move {
        exec_commands(&commands, command, response).await;
    });

    pending::<()>().await;

    Ok(())
}
