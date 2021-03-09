use git2::{FileFavor, MergeOptions, Oid, RebaseOptions, Repository, ResetType, Tree, Cred, CredentialType};
use std::path::Path;
use std::env;

mod error;
pub use error::Error;

lazy_static! {
    static ref MIRROR: &'static str = Box::leak(env::var("MIRROR").unwrap().into_boxed_str());
    static ref GIT_EMAIL: &'static str = Box::leak(env::var("GIT_EMAIL").unwrap().into_boxed_str());
    static ref GIT_PASS: &'static str = Box::leak(env::var("GIT_PASS").unwrap().into_boxed_str());
    static ref PROXY: &'static str = Box::leak(env::var("PROXY").unwrap().into_boxed_str());
}

pub trait EasyGit {
    fn add<P: AsRef<Path>>(&self, path: P) -> Result<Tree<'_>, Error>;
    fn reset_origin_hard(&self) -> Result<(), Error>;
    fn commit_message<M: AsRef<str>>(&self, message: M, tree: &Tree<'_>) -> Result<Oid, Error>;
    fn fetch_origin(&self) -> Result<(), Error>;
    fn rebase_master(&self) -> Result<(), Error>;
    fn try_merge(&self) -> Result<(), Error>;
    fn progress_monitor(progress: git2::Progress) -> bool;
    fn credentials(
        url: &str,
        user_from_url: Option<&str>,
        cred: git2::CredentialType,
        origin_config: &OriginConfig,
    ) -> Result<git2::Cred, git2::Error>;
}

#[derive(Debug, Clone)]
pub struct OriginConfig {
    pub url: String,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl EasyGit for Repository {
    /// git add $path
    fn add<P: AsRef<Path>>(&self, path: P) -> Result<Tree<'_>, Error> {
        let mut index = self.index()?;
        index.add_path(path.as_ref())?;
        index.write()?;
        let tree_id = index.write_tree()?;
        Ok(self.find_tree(tree_id)?)
    }

    /// git reset --hard origin/HEAD
    fn reset_origin_hard(&self) -> Result<(), Error> {
        let remote = self.find_reference("refs/remotes/origin/HEAD")?;
        self.reset(remote.peel_to_commit()?.as_object(), ResetType::Hard, None)?;
        Ok(())
    }

    /// git commit -m $message
    fn commit_message<M: AsRef<str>>(&self, message: M, tree: &Tree<'_>) -> Result<Oid, Error> {
        let head = self.head()?;
        let head_oid = head.target().ok_or(Error::SymbolicReference)?;
        let parent = self.find_commit(head_oid)?;
        let sig = self.signature()?;
        let oid = self.commit(
            Some("HEAD"),
            &sig,
            &sig,
            message.as_ref(),
            &tree,
            &[&parent],
        )?;
        Ok(oid)
    }

    /// git fetch origin/master
    fn fetch_origin(&self) -> Result<(), Error> {
        let mut origin = self.find_remote("origin")?;
        let mut proxy_options = git2::ProxyOptions::new();
        let mut fetch_options = git2::FetchOptions::new();
        if let Some(proxy) = Some(&PROXY) {
            proxy_options.url(proxy);
            fetch_options.proxy_options(proxy_options);
        }
        origin.fetch(&["master"], Some(&mut fetch_options), None)?;
        Ok(())
    }

    /// git rebase master origin/master
    fn rebase_master(&self) -> Result<(), Error> {
        let local = self.find_reference("refs/heads/master")?;
        let local_commit = self.reference_to_annotated_commit(&local)?;
        trace!("{:?} at: {:?}", local_commit.refname(), local_commit.id());
        let remote = self.find_reference("refs/remotes/origin/master")?;
        let remote_commit = self.reference_to_annotated_commit(&remote)?;
        trace!("{:?} at: {:?}", remote_commit.refname(), remote_commit.id());
        let mut rebase_options = RebaseOptions::new();
        let mut merge_options = MergeOptions::new();
        merge_options.file_favor(FileFavor::Theirs);
        rebase_options.merge_options(merge_options);
        let mut rebase = self.rebase(
            Some(&local_commit),
            Some(&remote_commit),
            None,
            Some(&mut rebase_options),
        )?;
        while let Some(r) = rebase.next() {
            let ro = r?;
            trace!("RebaseOperation: {:?} {:?}", ro.kind(), ro.id());
        }
        let oid = rebase.commit(None, &self.signature()?, None)?;
        trace!("Rebase commit at: {:?}", oid);
        rebase.finish(None)?;
        Ok(())
    }

    /// git merge
    fn try_merge(&self) -> Result<(), Error> {
        let local = self.find_reference("refs/heads/master")?;
        let local_commit = self.reference_to_annotated_commit(&local)?;
        trace!("{:?} at: {:?}", local_commit.refname(), local_commit.id());
        let head = self.head()?;
        let parent = self.find_commit(head.target().unwrap())?;
        let remote = self.find_reference("refs/remotes/origin/master")?;
        let remote_commit = self.reference_to_annotated_commit(&remote)?;
        trace!("{:?} at: {:?}", remote_commit.refname(), remote_commit.id());
        let c = self.reference_to_annotated_commit(&remote)?;
        let mut checkout = git2::build::CheckoutBuilder::new();
        let mut merge_option = git2::MergeOptions::new();
        let mut index = self.index()?;
        let old_tree = self.find_tree(index.write_tree()?)?;
        self.merge(&[&c],
                   Some(merge_option.file_favor(git2::FileFavor::Theirs)),
                   Some(checkout.force()),
        )?;
        index.write()?;
        let tree_id = index.write_tree()?;
        trace!("new commit at: {:?}", tree_id);
        let tree = self.find_tree(tree_id)?;
        let diff = self.diff_tree_to_tree(Some(&old_tree), Some(&tree), None)?;
        if diff.stats()?.files_changed() > 0 {
            let sig = self.signature()?;
            self.commit(Some("HEAD"), &sig, &sig, "Merge", &tree, &[&parent])?;
            let origin_config = OriginConfig {
                username: Some(GIT_EMAIL.to_string()),
                password: Some(GIT_PASS.to_string()),
                url: MIRROR.to_string()
            };
            if let Some(ref remote) = Some(origin_config) {
                let mut callbacks = git2::RemoteCallbacks::new();
                callbacks.credentials(|url, user, cred| Self::credentials(url, user, cred, remote));
                callbacks.transfer_progress(Self::progress_monitor);
                let mut remote = self.find_remote("local")?;
                let mut opts = git2::PushOptions::new();
                opts.remote_callbacks(callbacks);
                remote.push(&["refs/heads/master"], Some(&mut opts))?;
            }
            debug!("updated index");
        } else {
            trace!("Nothing to update");
            self.cleanup_state()?;
        }
        Ok(())
    }

    /// transfer progress
    fn progress_monitor(progress: git2::Progress) -> bool {
        debug!(
            "total :{}, local: {}, remote: {}",
            progress.total_objects(),
            progress.local_objects(),
            progress.received_objects()
        );
        true
    }

    fn credentials(
        url: &str,
        user_from_url: Option<&str>,
        cred: git2::CredentialType,
        origin_config: &OriginConfig,
    ) -> Result<git2::Cred, git2::Error> {
        let mut error = git2::Error::from_str(&format!("Failed to find credentials for {}", url));
        debug!("credentials");
        if cred.contains(CredentialType::USER_PASS_PLAINTEXT) {
            if let (&Some(ref u), &Some(ref p)) = (&origin_config.username, &origin_config.password) {
                debug!("from username/password: {}:{}", u, p);
                match Cred::userpass_plaintext(u, p) {
                    Err(e) => {
                        debug!("Error: {:?}", e);
                        error = e;
                    }
                    Ok(c) => {
                        debug!("Ok!");
                        return Ok(c);
                    }
                }
            }
        }
        if cred.contains(CredentialType::DEFAULT) {
            let config = git2::Config::open_default()?;
            match Cred::credential_helper(&config, url, user_from_url) {
                Err(e) => {
                    debug!("Error: {:?}", e);
                    error = e;
                }
                Ok(c) => {
                    debug!("Ok!");
                    return Ok(c);
                }
            }
        }
        if cred.contains(CredentialType::SSH_KEY) {
            if let Some(user) = user_from_url {
                debug!("from ssh agent");
                match Cred::ssh_key_from_agent(user) {
                    Err(e) => {
                        debug!("Error: {:?}", e);
                        error = e;
                    }
                    Ok(c) => {
                        debug!("Ok");
                        return Ok(c);
                    }
                }
            }
        }
        Err(error)
    }
}
