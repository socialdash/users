use std::sync::Arc;

use futures::future;
use futures::Future;
use futures_cpupool::CpuPool;
use sha3::{Digest, Sha3_256};
use rand;
use base64::encode;

use models::user::{NewUser, UpdateUser, User};
use models::identity::{NewIdentity, Provider};
use repos::identities::{IdentitiesRepo, IdentitiesRepoImpl};
use repos::users::{UsersRepo, UsersRepoImpl};

use super::types::ServiceFuture;
use super::error::Error;
use repos::types::DbPool;
use repos::acl::{ApplicationAcl, RolesCacheImpl, Acl, UnAuthanticatedACL};


pub trait UsersService {
    /// Returns user by ID
    fn get(&self, user_id: i32) -> ServiceFuture<User>;
    /// Returns current user
    fn current(&self) -> ServiceFuture<User>;
    /// Lists users limited by `from` and `count` parameters
    fn list(&self, from: i32, count: i64) -> ServiceFuture<Vec<User>>;
    /// Deactivates specific user
    fn deactivate(&self, user_id: i32) -> ServiceFuture<User>;
    /// Creates new user
    fn create(&self, payload: NewIdentity) -> ServiceFuture<User>;
    /// Updates specific user
    fn update(&self, user_id: i32, payload: UpdateUser) -> ServiceFuture<User>;
    /// creates hashed password
    fn password_create(clear_password: String) -> String;
}

/// Users services, responsible for User-related CRUD operations
pub struct UsersServiceImpl<
    U: 'static + UsersRepo + Clone,
    I: 'static + IdentitiesRepo + Clone,
> {
    pub users_repo: U,
    pub ident_repo: I,
    pub user_id: Option<i32>,
}

impl UsersServiceImpl<UsersRepoImpl, IdentitiesRepoImpl> {
    pub fn new(
        db_pool: DbPool,
        cpu_pool: CpuPool,
        roles_cache: RolesCacheImpl,
        user_id: Option<i32>,
    ) -> Self {
        let ident_repo = IdentitiesRepoImpl::new(db_pool.clone(), cpu_pool.clone());
        let acl =  user_id.map_or((Arc::new(UnAuthanticatedACL::new()) as Arc<Acl>), |id| (Arc::new(ApplicationAcl::new(roles_cache.clone(), id)) as Arc<Acl>));
        let users_repo = UsersRepoImpl::new(db_pool, cpu_pool, acl);
        Self {
            users_repo: users_repo,
            ident_repo: ident_repo,
            user_id: user_id,
        }
    }

}

impl<
    U: 'static + UsersRepo + Clone,
    I: 'static + IdentitiesRepo + Clone,
> UsersService for UsersServiceImpl<U, I> {
    /// Returns user by ID
    fn get(&self, user_id: i32) -> ServiceFuture<User> {
        Box::new(
            self.users_repo
                .find(user_id)
                .map_err(Error::from)
                )
    }

    /// Returns current user
    fn current(&self) -> ServiceFuture<User> {
        if let Some(id) = self.user_id {
            Box::new(self.users_repo.find(id).map_err(Error::from))
        } else {
            Box::new(future::err(Error::Unknown(
                format!("There is no user id in request header."),
            )))
        }
    }

    /// Lists users limited by `from` and `count` parameters
    fn list(&self, from: i32, count: i64) -> ServiceFuture<Vec<User>> {
        Box::new(
            self.users_repo
                .list(from, count)
                .map_err(|e| Error::from(e)),
        )
    }

    /// Deactivates specific user
    fn deactivate(&self, user_id: i32) -> ServiceFuture<User> {
        Box::new(
            self.users_repo
                .deactivate(user_id)
                .map_err(|e| Error::from(e)),
        )
    }

    /// Creates new user
    fn create(&self, payload: NewIdentity) -> ServiceFuture<User> {
        let users_repo = self.users_repo.clone();
        let ident_repo = self.ident_repo.clone();
        Box::new(
            ident_repo
                .email_provider_exists(payload.email.to_string(), Provider::Email)
                .map(move |exists| (payload, exists))
                .map_err(Error::from)
                .and_then(|(payload, exists)| match exists {
                    false => future::ok(payload),
                    true => future::err(Error::Validate(
                        validation_errors!({"email": ["email" => "Email already exists"]}),
                    )),
                })
                .and_then(move |new_ident| {
                    let new_user = NewUser::from(new_ident.clone());
                    users_repo
                        .create(new_user)
                        .map_err(|e| Error::from(e))
                        .map(|user| (new_ident, user))
                })
                .and_then(move |(new_ident, user)| {
                    ident_repo
                        .create(
                            new_ident.email,
                            Some(Self::password_create(new_ident.password.clone())),
                            Provider::Email,
                            user.id,
                        )
                        .map_err(|e| Error::from(e))
                        .map(|_| user)
                }),
        )
    }

    /// Updates specific user
    fn update(&self, user_id: i32, payload: UpdateUser) -> ServiceFuture<User> {
        let users_repo = self.users_repo.clone();

        Box::new(
            users_repo
                .find(user_id)
                .and_then(move |_user| users_repo.update(user_id, payload))
                .map_err(|e| Error::from(e)),
        )
    }

    fn password_create(clear_password: String) -> String {
        let salt = rand::random::<u64>().to_string().split_off(10);
        let pass = clear_password + &salt;
        let mut hasher = Sha3_256::default();
        hasher.input(pass.as_bytes());
        let out = hasher.result();
        let computed_hash = encode(&out[..]);
        computed_hash + "." + &salt
    }
}



