from typing import cast

from torch.nn import Module

# from torch.optim.lr_scheduler import LRScheduler
from transformers import (
    AutoModel,
    AutoModelForAudioClassification,
    AutoModelForAudioFrameClassification,
    AutoModelForAudioTokenization,
    AutoModelForAudioXVector,
    AutoModelForCausalLM,
    AutoModelForCTC,
    AutoModelForDepthEstimation,
    AutoModelForDocumentQuestionAnswering,
    AutoModelForImageClassification,
    AutoModelForImageSegmentation,
    AutoModelForImageTextToText,
    AutoModelForImageToImage,
    AutoModelForInstanceSegmentation,
    AutoModelForKeypointDetection,
    AutoModelForKeypointMatching,
    AutoModelForMaskedLM,
    AutoModelForMaskGeneration,
    AutoModelForMultipleChoice,
    AutoModelForNextSentencePrediction,
    AutoModelForObjectDetection,
    AutoModelForPreTraining,
    AutoModelForQuestionAnswering,
    AutoModelForSemanticSegmentation,
    AutoModelForSeq2SeqLM,
    AutoModelForSequenceClassification,
    AutoModelForSpeechSeq2Seq,
    AutoModelForTextEncoding,
    AutoModelForTextToSpectrogram,
    AutoModelForTextToWaveform,
    AutoModelForTimeSeriesPrediction,
    AutoModelForTokenClassification,
    AutoModelForUniversalSegmentation,
    AutoModelForVideoClassification,
    AutoModelForVision2Seq,
    AutoModelForZeroShotImageClassification,
    AutoModelForZeroShotObjectDetection,
)


def get_model(model_path: str, model_type: str) -> Module:  # noqa: PLR0911, PLR0912
    # Initializing a model from a Hugging Face configuration
    if model_type == "auto":
        return cast(Module, AutoModel.from_pretrained(model_path, trust_remote_code=True))
    if model_type == "pretraining":
        return cast(Module, AutoModelForPreTraining.from_pretrained(model_path, trust_remote_code=True))
    if model_type == "causal-lm":
        return cast(Module, AutoModelForCausalLM.from_pretrained(model_path, trust_remote_code=True))
    if model_type == "masked-lm":
        return cast(Module, AutoModelForMaskedLM.from_pretrained(model_path, trust_remote_code=True))
    if model_type == "mask-generation":
        return cast(Module, AutoModelForMaskGeneration.from_pretrained(model_path, trust_remote_code=True))
    if model_type == "seq-2-seq-lm":
        return cast(Module, AutoModelForSeq2SeqLM.from_pretrained(model_path, trust_remote_code=True))
    if model_type == "sequence-classification":
        return cast(Module, AutoModelForSequenceClassification.from_pretrained(model_path, trust_remote_code=True))
    if model_type == "multiple-choice":
        return cast(Module, AutoModelForMultipleChoice.from_pretrained(model_path, trust_remote_code=True))
    if model_type == "next-sentence-prediction":
        return cast(Module, AutoModelForNextSentencePrediction.from_pretrained(model_path, trust_remote_code=True))
    if model_type == "token-classification":
        return cast(Module, AutoModelForTokenClassification.from_pretrained(model_path, trust_remote_code=True))
    if model_type == "question-answering":
        return cast(Module, AutoModelForQuestionAnswering.from_pretrained(model_path, trust_remote_code=True))
    if model_type == "text-encoding":
        return cast(Module, AutoModelForTextEncoding.from_pretrained(model_path, trust_remote_code=True))
    if model_type == "depth-estimation":
        return cast(Module, AutoModelForDepthEstimation.from_pretrained(model_path, trust_remote_code=True))
    if model_type == "image-classification":
        return cast(Module, AutoModelForImageClassification.from_pretrained(model_path, trust_remote_code=True))
    if model_type == "video-classification":
        return cast(Module, AutoModelForVideoClassification.from_pretrained(model_path, trust_remote_code=True))
    if model_type == "keypoint-detection":
        return cast(Module, AutoModelForKeypointDetection.from_pretrained(model_path, trust_remote_code=True))
    if model_type == "keypoint-matching":
        return cast(Module, AutoModelForKeypointMatching.from_pretrained(model_path, trust_remote_code=True))
    if model_type == "object-detection":
        return cast(Module, AutoModelForObjectDetection.from_pretrained(model_path, trust_remote_code=True))
    if model_type == "image-segmentation":
        return cast(Module, AutoModelForImageSegmentation.from_pretrained(model_path, trust_remote_code=True))
    if model_type == "image-to-image":
        return cast(Module, AutoModelForImageToImage.from_pretrained(model_path, trust_remote_code=True))
    if model_type == "semantic-segmentation":
        return cast(Module, AutoModelForSemanticSegmentation.from_pretrained(model_path, trust_remote_code=True))
    if model_type == "instance-segmentation":
        return cast(Module, AutoModelForInstanceSegmentation.from_pretrained(model_path, trust_remote_code=True))
    if model_type == "universal-segmentation":
        return cast(Module, AutoModelForUniversalSegmentation.from_pretrained(model_path, trust_remote_code=True))
    if model_type == "zero-shot-image-classification":
        return cast(Module, AutoModelForZeroShotImageClassification.from_pretrained(model_path, trust_remote_code=True))
    if model_type == "zero-shot-object-detection":
        return cast(Module, AutoModelForZeroShotObjectDetection.from_pretrained(model_path, trust_remote_code=True))
    if model_type == "audio-classification":
        return cast(Module, AutoModelForAudioClassification.from_pretrained(model_path, trust_remote_code=True))
    if model_type == "audio-frame-classification":
        return cast(Module, AutoModelForAudioFrameClassification.from_pretrained(model_path, trust_remote_code=True))
    if model_type == "ctc":
        return cast(Module, AutoModelForCTC.from_pretrained(model_path, trust_remote_code=True))
    if model_type == "speech-seq-2-seq":
        return cast(Module, AutoModelForSpeechSeq2Seq.from_pretrained(model_path, trust_remote_code=True))
    if model_type == "audio-x-vector":
        return cast(Module, AutoModelForAudioXVector.from_pretrained(model_path, trust_remote_code=True))
    if model_type == "text-to-spectrogram":
        return cast(Module, AutoModelForTextToSpectrogram.from_pretrained(model_path, trust_remote_code=True))
    if model_type == "text-to-waveform":
        return cast(Module, AutoModelForTextToWaveform.from_pretrained(model_path, trust_remote_code=True))
    if model_type == "audio-tokenization":
        return cast(Module, AutoModelForAudioTokenization.from_pretrained(model_path, trust_remote_code=True))
    if model_type == "table-question-answering":
        return cast(Module, AutoModelForQuestionAnswering.from_pretrained(model_path, trust_remote_code=True))
    if model_type == "document-question-answering":
        return cast(Module, AutoModelForDocumentQuestionAnswering.from_pretrained(model_path, trust_remote_code=True))
    if model_type == "vision-2-seq":
        return cast(Module, AutoModelForVision2Seq.from_pretrained(model_path, trust_remote_code=True))  # type: ignore [no-untyped-call]
    if model_type == "image-text-to-text":
        return cast(Module, AutoModelForImageTextToText.from_pretrained(model_path, trust_remote_code=True))
    if model_type == "time-series-prediction":
        return cast(Module, AutoModelForTimeSeriesPrediction.from_pretrained(model_path, trust_remote_code=True))

    raise RuntimeError(f"Model type {model_type} not supported.")
