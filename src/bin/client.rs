use std::num::NonZeroUsize;
use std::os::fd::BorrowedFd;
use tokio::time::{Duration, sleep};

use zbus::{Connection, fdo::Error as ZBusError, proxy};

use rtipc::{ChannelAttr, ChannelGroup, Consumer, GroupAttr, PopResult, Producer, QueueAttr};

use rtipc_zbus::{AsyncEventFd, CommandId, MsgCommand, MsgEvent, MsgResponse};

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
    async fn connect(&self, request: Vec<u8>, fds: Vec<zvariant::OwnedFd>)
    -> Result<(), ZBusError>;
}

async fn listen_events(mut event: Consumer<MsgEvent>) {
    for _ in 0..1000 {
        sleep(Duration::from_millis(1)).await;
        loop {
            match event.pop() {
                PopResult::QueueError => panic!(),
                PopResult::NoMessage => {
                    break;
                }
                PopResult::NoNewMessage => {
                    break;
                }
                PopResult::Success => {
                    println!(
                        "client received event: {}",
                        event.current_message().unwrap()
                    )
                }
                PopResult::SuccessMessagesDiscarded => {
                    println!(
                        "client received event: {}",
                        event.current_message().unwrap()
                    )
                }
            }
        }
    }
    println!("listen_events returns");
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
            PopResult::QueueError => panic!(),
            PopResult::NoMessage => {
                continue;
            }
            PopResult::NoNewMessage => {
                continue;
            }
            PopResult::Success => {}
            PopResult::SuccessMessagesDiscarded => {}
        };

        println!(
            "client received response: {}",
            response.current_message().unwrap()
        );
    }
    println!("all commands executed");
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

    let c2s_channels: [ChannelAttr; 1] = [ChannelAttr {
        queue: QueueAttr {
            additional_messages: 0,
            message_size: unsafe { NonZeroUsize::new_unchecked(size_of::<MsgCommand>()) },
        },
        eventfd: true,
        info: b"rpc command".to_vec(),
    }];

    let s2c_channels: [ChannelAttr; 2] = [
        ChannelAttr {
            queue: QueueAttr {
                additional_messages: 0,
                message_size: unsafe { NonZeroUsize::new_unchecked(size_of::<MsgResponse>()) },
            },
            eventfd: true,
            info: b"rpc response".to_vec(),
        },
        ChannelAttr {
            queue: QueueAttr {
                additional_messages: 10,
                message_size: unsafe { NonZeroUsize::new_unchecked(size_of::<MsgEvent>()) },
            },
            eventfd: false,
            info: b"rpc event".to_vec(),
        },
    ];

    let attr = GroupAttr {
        producers: c2s_channels.to_vec(),
        consumers: s2c_channels.to_vec(),
        info: b"rpc example".to_vec(),
    };

    let mut grp = ChannelGroup::from_attr(&attr)
        .map_err(|_| ZBusError::InvalidArgs(String::from("ChannelGroup::new failed")))?;
    let (request, bfds) = grp.serialize();

    let fds: Vec<zvariant::OwnedFd> = bfds
        .into_iter()
        .map(to_owned_fd)
        .collect::<Result<Vec<zvariant::OwnedFd>, ZBusError>>()?;

    let connection = Connection::session().await?;

    let proxy = ServerProxy::new(&connection).await?;
    proxy.connect(request, fds).await?;

    let command = grp.take_producer(0).unwrap();
    let response = grp.take_consumer(0).unwrap();
    let event = grp.take_consumer(1).unwrap();

    let event_task = tokio::spawn(async move {
        listen_events(event).await;
    });

    let command_task = tokio::spawn(async move {
        exec_commands(&commands, command, response).await;
    });

    event_task.await.unwrap();
    command_task.await.unwrap();

    Ok(())
}
