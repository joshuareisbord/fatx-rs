FROM rust:1-bookworm

RUN apt-get update && apt-get install -y \
    git \
    lldb \
    pkg-config \
    && rm -rf /var/lib/apt/lists/*

# Install common cargo tools
RUN cargo install cargo-watch cargo-expand

# Create non-root user
ARG USERNAME=vscode
ARG USER_UID=1000
ARG USER_GID=$USER_UID
RUN groupadd --gid $USER_GID $USERNAME \
    && useradd --uid $USER_UID --gid $USER_GID -m $USERNAME \
    && mkdir -p /home/$USERNAME/.cargo \
    && chown -R $USERNAME:$USERNAME /home/$USERNAME

USER $USERNAME
WORKDIR /workspaces/fatx-rs
