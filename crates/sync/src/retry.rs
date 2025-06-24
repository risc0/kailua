// Copyright 2025 RISC Zero, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

#[macro_export]
macro_rules! retry {
    ($e:expr) => {
        $crate::retry!(250, 1000, $e)
    };
    ($m:literal, $e:expr) => {
        $crate::retry!(250, $m, $e)
    };
    ($b:literal, $m:literal, $e:expr) => {
        tokio_retry::Retry::spawn(
            tokio_retry::strategy::ExponentialBackoff::from_millis($b)
                .max_delay(std::time::Duration::from_millis($m)),
            || async {
                let res = $e;
                if let Err(err) = &res {
                    tracing::error!("(Retrying) {err:?}");
                }
                res
            },
        )
    };
}

#[macro_export]
macro_rules! retry_res {
    ($e:expr) => {
        $crate::retry_res!(250, 1000, $e)
    };
    ($m:literal, $e:expr) => {
        $crate::retry_res!(250, $m, $e)
    };
    ($b:literal, $m:literal, $e:expr) => {
        async { $crate::retry!($b, $m, $e).await.unwrap() }
    };
}

#[macro_export]
macro_rules! retry_ctx {
    ($e:expr) => {
        $crate::retry_ctx!(250, 1000, $e)
    };
    ($m:literal, $e:expr) => {
        $crate::retry_ctx!(250, $m, $e)
    };
    ($b:literal, $m:literal, $e:expr) => {
        $crate::retry!(
            $b,
            $m,
            opentelemetry::trace::FutureExt::with_context(
                $e,
                opentelemetry::Context::current_with_span(
                    opentelemetry::global::tracer("kailua")
                        .start_with_context("retry_attempt", &opentelemetry::Context::current()),
                )
            )
            .await
        )
    };
}

#[macro_export]
macro_rules! retry_res_ctx {
    ($e:expr) => {
        $crate::retry_res_ctx!(250, 1000, $e)
    };
    ($m:literal, $e:expr) => {
        $crate::retry_res_ctx!(250, $m, $e)
    };
    ($b:literal, $m:literal, $e:expr) => {
        async { $crate::retry_ctx!($b, $m, $e).await.unwrap() }
    };
}

#[macro_export]
macro_rules! retry_timeout {
    ($e:expr) => {
        $crate::retry_timeout!(2, 250, 1000, $e)
    };
    ($t:expr, $e:expr) => {
        $crate::retry_timeout!($t, 250, 1000, $e)
    };
    ($t:expr, $m:literal, $e:expr) => {
        $crate::retry_timeout!($t, 250, $m, $e)
    };
    ($t:expr, $b:literal, $m:literal, $e:expr) => {
        $crate::retry_res!(
            $b,
            $m,
            tokio::time::timeout(core::time::Duration::from_secs($t), async { $e }).await
        )
    };
}

#[macro_export]
macro_rules! retry_res_timeout {
    ($e:expr) => {
        $crate::retry_res_timeout!(2, 250, 1000, $e)
    };
    ($t:expr, $e:expr) => {
        $crate::retry_res_timeout!($t, 250, 1000, $e)
    };
    ($t:expr, $m:literal, $e:expr) => {
        $crate::retry_res_timeout!($t, 250, $m, $e)
    };
    ($t:expr, $b:literal, $m:literal, $e:expr) => {
        async {
            $crate::retry_res!(
                $crate::retry_res!(
                    $b,
                    $m,
                    tokio::time::timeout(core::time::Duration::from_secs($t), async { $e }).await
                )
                .await
            )
            .await
        }
    };
}

#[macro_export]
macro_rules! retry_res_ctx_timeout {
    ($e:expr) => {
        $crate::retry_res_ctx_timeout!(2, 250, 1000, $e)
    };
    ($t:expr, $e:expr) => {
        $crate::retry_res_ctx_timeout!($t, 250, 1000, $e)
    };
    ($t:expr, $m:literal, $e:expr) => {
        $crate::retry_res_ctx_timeout!($t, 250, $m, $e)
    };
    ($t:expr, $b:literal, $m:literal, $e:expr) => {
        async {
            $crate::retry_res_ctx!($crate::retry_res_ctx!(
                $b,
                $m,
                tokio::time::timeout(core::time::Duration::from_secs($t), async { $e })
            ))
            .await
        }
    };
}
